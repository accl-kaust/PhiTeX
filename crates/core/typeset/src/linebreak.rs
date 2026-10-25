//! Breaking paragraphs into lines (§813–§890), with e-TeX's
//! `\lastlinefit` and penalty arrays (etex.ch).
//!
//! tex.web keeps "delta nodes" between active breakpoints so that its
//! 32-bit sums never overflow. Here every active breakpoint stores its
//! position `alpha` as a 64-bit total from the start of the paragraph;
//! the width of a line from an active node to the current point is the
//! same integer tex.web computes from its deltas (§824), so every
//! decision is the same.

use crate::arith::{INF_BAD, MAX_DIMEN, Scaled, badness, fract};
use crate::fonts::{FontId, Fonts};
use crate::glue::{Glue, Order};
use crate::node::{List, Node, param};
use crate::pack::{PackParams, Packed, Spec, hpack};

/// `inf_penalty` (§157).
pub const INF_PENALTY: i32 = 10000;
/// `eject_penalty` (§157).
pub const EJECT_PENALTY: i32 = -INF_PENALTY;
/// `awful_bad` (§833).
pub const AWFUL_BAD: i32 = 0o7777777777;
/// web2c's `max_halfword`: the line number of `last_active` (§820).
const MAX_HALFWORD: i32 = 0xFFF_FFFF;

const VERY_LOOSE_FIT: usize = 0;
const LOOSE_FIT: usize = 1;
const DECENT_FIT: usize = 2;
const TIGHT_FIT: usize = 3;

/// The hyphenation language in force (`cur_lang`, `l_hyf`, `r_hyf`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LangState {
    /// `\language`.
    pub lang: u16,
    /// `\lefthyphenmin`, normalized.
    pub lhm: u8,
    /// `\righthyphenmin`, normalized.
    pub rhm: u8,
}

/// The second pass's hook (§866 "Try to hyphenate the following word"):
/// asked about the glue at `list[at]` when breaks are automatic; it may
/// rewrite the word that follows (nodes after `at` only).
pub trait Hyphenator {
    /// The hyphenated form of the word after `list[at]`: the nodes that
    /// replace `list[from..=to]`, as `(from, to, nodes)`; `None` leaves
    /// the list as it is.
    fn hyphenate(
        &mut self,
        list: &[Node],
        at: usize,
        lang: LangState,
        fonts: &dyn Fonts,
    ) -> Option<(usize, usize, List)>;
}

/// No hyphenation.
pub struct NoHyphenation;

impl Hyphenator for NoHyphenation {
    fn hyphenate(
        &mut self,
        _: &[Node],
        _: usize,
        _: LangState,
        _: &dyn Fonts,
    ) -> Option<(usize, usize, List)> {
        None
    }
}

/// The words of `list` hyphenated as the second pass meets them (§866,
/// §891–§918), all at once: a word's hyphenation reads only the word and
/// what follows it, and no break before it looks into it beyond its
/// first node (§837 stops there), so the passes see the same list as
/// when each word is hyphenated as its glue is passed. The glue, the
/// language and whether breaks are automatic are followed as the pass
/// follows them (no word holds a glue, language or math node).
fn hyphenate_all(
    mut list: List,
    lang: LangState,
    hyphenator: &mut dyn Hyphenator,
    fonts: &dyn Fonts,
    edits: &mut Vec<(usize, usize, List)>,
    spare: &mut List,
) -> List {
    edits.clear();
    let mut lang = lang;
    let mut auto_breaking = true;
    for (at, n) in list.iter().enumerate() {
        match n {
            Node::Language { lang: l, lhm, rhm } => {
                lang = LangState {
                    lang: *l,
                    lhm: *lhm,
                    rhm: *rhm,
                };
            }
            Node::Math(kind, _) => auto_breaking = *kind == crate::node::MathKind::After,
            Node::Glue(_) if auto_breaking => {
                if let Some(e) = hyphenator.hyphenate(&list, at, lang, fonts) {
                    edits.push(e);
                }
            }
            _ => {}
        }
    }
    if edits.is_empty() {
        return list;
    }
    let extra: usize = edits.iter().map(|e| e.2.len()).sum();
    // (the hyphenated list in the buffer the last one left, which then
    // keeps this one's)
    let mut out = std::mem::take(spare);
    out.clear();
    out.reserve(list.len() + extra);
    let mut edits = edits.drain(..).peekable();
    let mut skip_to = 0;
    for (k, n) in list.drain(..).enumerate() {
        if k < skip_to {
            continue;
        }
        if let Some((_, to, nodes)) = edits.next_if(|e| e.0 == k) {
            out.extend(nodes);
            skip_to = to + 1;
            continue;
        }
        out.push(n);
    }
    *spare = list;
    out
}

/// The parameters `line_break` reads from eqtb.
#[derive(Clone, Debug, PartialEq)]
pub struct LineBreakParams {
    /// `\pretolerance`.
    pub pretolerance: i32,
    /// `\tolerance`.
    pub tolerance: i32,
    /// `\emergencystretch`.
    pub emergency_stretch: Scaled,
    /// `\looseness`.
    pub looseness: i32,
    /// `\linepenalty`.
    pub line_penalty: i32,
    /// `\hyphenpenalty`.
    pub hyphen_penalty: i32,
    /// `\exhyphenpenalty`.
    pub ex_hyphen_penalty: i32,
    /// `\adjdemerits`.
    pub adj_demerits: i32,
    /// `\doublehyphendemerits`.
    pub double_hyphen_demerits: i32,
    /// `\finalhyphendemerits`.
    pub final_hyphen_demerits: i32,
    /// `\leftskip`.
    pub left_skip: Glue,
    /// `\rightskip`.
    pub right_skip: Glue,
    /// `\parfillskip`.
    pub par_fill_skip: Glue,
    /// `\hsize`.
    pub hsize: Scaled,
    /// `\hangindent`.
    pub hang_indent: Scaled,
    /// `\hangafter`.
    pub hang_after: i32,
    /// `\parshape` as (indent, width) pairs; empty when null.
    pub par_shape: Vec<(Scaled, Scaled)>,
    /// `\lastlinefit` (e-TeX).
    pub last_line_fit: i32,
    /// `\interlinepenalty`.
    pub inter_line_penalty: i32,
    /// `\clubpenalty`.
    pub club_penalty: i32,
    /// `\widowpenalty`.
    pub widow_penalty: i32,
    /// `\displaywidowpenalty`.
    pub display_widow_penalty: i32,
    /// `\brokenpenalty`.
    pub broken_penalty: i32,
    /// `\interlinepenalties` (e-TeX); empty when null.
    pub inter_line_penalties: Vec<i32>,
    /// `\clubpenalties` (e-TeX).
    pub club_penalties: Vec<i32>,
    /// `\widowpenalties` (e-TeX).
    pub widow_penalties: Vec<i32>,
    /// `\displaywidowpenalties` (e-TeX).
    pub display_widow_penalties: Vec<i32>,
    /// `prev_graf` of the enclosing vertical list.
    pub prev_graf: i32,
    /// The language at the start of the paragraph (`init_cur_lang`, ...).
    pub lang: LangState,
    /// `\hbadness`, `\hfuzz`, `\overfullrule` for the lines.
    pub pack: PackParams,
    /// `\pdfadjustspacing` (pdfTeX): font expansion in the breaking (above
    /// 1) and the lines' packing (above 0).
    pub adjust_spacing: i32,
    /// `\pdfprotrudechars` (pdfTeX): character protrusion in the breaking
    /// (above 1) and the lines (above 0).
    pub protrude_chars: i32,
}

impl Default for LineBreakParams {
    /// plain TeX's values with `\hsize=6.5in`.
    fn default() -> Self {
        LineBreakParams {
            pretolerance: 100,
            tolerance: 200,
            emergency_stretch: 0,
            looseness: 0,
            line_penalty: 10,
            hyphen_penalty: 50,
            ex_hyphen_penalty: 50,
            adj_demerits: 10000,
            double_hyphen_demerits: 10000,
            final_hyphen_demerits: 5000,
            left_skip: Glue::ZERO,
            right_skip: Glue::ZERO,
            par_fill_skip: Glue {
                stretch: 0x1_0000,
                stretch_order: Order::Fil,
                ..Glue::ZERO
            },
            hsize: 30_828_134,
            hang_indent: 0,
            hang_after: 1,
            par_shape: Vec::new(),
            last_line_fit: 0,
            inter_line_penalty: 0,
            club_penalty: 150,
            widow_penalty: 150,
            display_widow_penalty: 50,
            broken_penalty: 100,
            inter_line_penalties: Vec::new(),
            club_penalties: Vec::new(),
            widow_penalties: Vec::new(),
            display_widow_penalties: Vec::new(),
            prev_graf: 0,
            lang: LangState {
                lang: 0,
                lhm: 2,
                rhm: 3,
            },
            pack: PackParams::default(),
            adjust_spacing: 0,
            protrude_chars: 0,
        }
    }
}

/// One line of the broken paragraph (§880–§890), in the order tex.web
/// appends to the vertical list: the box (`append_to_vlist`), its
/// adjustments, then the penalty if nonzero.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The packed line, shifted by its indentation.
    pub packed: Packed,
    /// The interline penalty after it, if nonzero.
    pub penalty: Option<i32>,
}

/// The broken paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    /// The lines.
    pub lines: Vec<Line>,
    /// The new `prev_graf` (`best_line - 1`).
    pub prev_graf: i32,
    /// tex.web's "Infinite glue shrinkage found in a paragraph" (§826).
    pub infinite_shrink_error: bool,
    /// The pass that succeeded: 1 first, 2 second, 3 emergency.
    pub pass: u8,
}

/// A line's edge character (pdfTeX's `last_leftmost_char`,
/// `last_rightmost_char`) and whether a ligature's.
type Edge = Option<(FontId, u16, bool)>;

/// tex.web's six widths and pdfTeX's font stretch and shrink (7, 8).
type Widths = [i64; 8];

/// A node of the paragraph as pdfTeX's list reaches it: the list's
/// `i`th, or the `j`th of the pre-break, post-break or replacement list
/// of the discretionary there (`prev_char_p`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pos {
    Main(usize),
    Pre(usize, usize),
    Post(usize, usize),
    Rep(usize, usize),
}

fn pos_node(list: &[Node], p: Pos) -> Option<&Node> {
    match p {
        Pos::Main(i) => list.get(i),
        Pos::Pre(i, j) | Pos::Post(i, j) | Pos::Rep(i, j) => {
            let Node::Disc(d) = list.get(i)? else {
                return None;
            };
            match p {
                Pos::Pre(..) => d.pre.get(j),
                Pos::Post(..) => d.post.get(j),
                _ => d.replace.get(j),
            }
        }
    }
}

/// `link(p)`: a discretionary's replacement follows it in pdfTeX's list.
fn pos_next(list: &[Node], p: Pos) -> Option<Pos> {
    let after = |i: usize| (i + 1 < list.len()).then_some(Pos::Main(i + 1));
    match p {
        Pos::Main(i) => match list.get(i)? {
            Node::Disc(d) if !d.replace.is_empty() => Some(Pos::Rep(i, 0)),
            _ => after(i),
        },
        Pos::Rep(i, j) => {
            let Node::Disc(d) = list.get(i)? else {
                return None;
            };
            if j + 1 < d.replace.len() {
                Some(Pos::Rep(i, j + 1))
            } else {
                after(i)
            }
        }
        Pos::Pre(i, j) | Pos::Post(i, j) => {
            let Node::Disc(d) = list.get(i)? else {
                return None;
            };
            let n = if matches!(p, Pos::Pre(..)) {
                d.pre.len()
            } else {
                d.post.len()
            };
            (j + 1 < n).then_some(if matches!(p, Pos::Pre(..)) {
                Pos::Pre(i, j + 1)
            } else {
                Pos::Post(i, j + 1)
            })
        }
    }
}

#[derive(Clone, Debug)]
struct Active {
    /// The passive node, `None` for the start of the paragraph.
    passive: Option<usize>,
    line_number: i32,
    fitness: usize,
    hyphenated: bool,
    total_demerits: i32,
    /// The width from the start of the paragraph to just after this break
    /// minus the background: `alpha - gamma` in §824.
    alpha: Widths,
    short: Scaled,
    glue: Scaled,
}

/// The vectors a paragraph's breaking fills, kept per thread from one
/// paragraph to the next (cleared, not freed).
#[derive(Default)]
struct Pool {
    active: Vec<Active>,
    passive: Vec<Passive>,
    edits: Vec<(usize, usize, List)>,
    breaks: Vec<Option<usize>>,
    /// Lists of lines already read, emptied ([`recycle_line`]).
    lines: Vec<List>,
    /// A paragraph's list before hyphenation, emptied.
    unhyphenated: List,
}

/// Hands the list of a line [`line_break`] made back, once read: kept
/// (emptied) for the lines of the thread's next paragraphs.
pub fn recycle_line(mut list: List) {
    list.clear();
    if list.capacity() == 0 {
        return;
    }
    POOL.with(|c| {
        if let Some(mut p) = c.take() {
            if p.lines.len() < 64 {
                p.lines.push(list);
            }
            c.set(Some(p));
        }
    });
}

thread_local! {
    static POOL: std::cell::Cell<Option<Box<Pool>>> = const { std::cell::Cell::new(None) };
}

#[derive(Clone, Copy, Debug)]
struct Passive {
    cur_break: Option<usize>,
    prev_break: Option<usize>,
}

fn add_glue(w: &mut Widths, g: &Glue, sign: i64) {
    w[0] += sign * i64::from(g.width);
    w[1 + g.stretch_order as usize] += sign * i64::from(g.stretch);
    w[5] += sign * i64::from(g.shrink);
}

fn to_scaled(x: i64) -> Scaled {
    // tex.web keeps these in 32 bits; wrap as it would.
    #[allow(clippy::cast_possible_truncation)]
    {
        x as i32
    }
}

/// The width of a node in a discretionary or its replacement (§841, §842,
/// §870, §871).
fn disc_item_width(n: &Node, fonts: &dyn Fonts) -> Scaled {
    match n {
        Node::Char { font, ch } => fonts.char_width(*font, *ch),
        Node::Lig(l) => fonts.char_width(l.font, l.ch),
        Node::Box(_) | Node::Rule(_) | Node::Kern(..) => n.box_dims().map_or_else(
            || match n {
                Node::Kern(w, _) => *w,
                _ => 0,
            },
            |d| d.0,
        ),
        _ => panic!("line_break: improper discretionary material"),
    }
}

struct Breaker<'a> {
    p: &'a LineBreakParams,
    fonts: &'a dyn Fonts,
    background: Widths,
    /// beta: total from the start of the paragraph to the current point.
    total: Widths,
    active: Vec<Active>,
    passive: Vec<Passive>,
    minimal_demerits: [i32; 4],
    minimum_demerits: i32,
    best_place: [Option<usize>; 4],
    best_pl_line: [i32; 4],
    best_pl_short: [Scaled; 4],
    best_pl_glue: [Scaled; 4],
    threshold: i32,
    second_pass: bool,
    final_pass: bool,
    disc_width: Scaled,
    easy_line: i32,
    last_special_line: i32,
    first_width: Scaled,
    second_width: Scaled,
    first_indent: Scaled,
    second_indent: Scaled,
    do_last_line_fit: bool,
    fill_width: [Scaled; 3],
    /// pdfTeX's `disc_width[7]`, `disc_width[8]`.
    disc_fw: [Scaled; 2],
    /// pdfTeX's `prev_char_p`.
    prev_char: Option<Pos>,
    /// pdfTeX's `max_stretch_ratio`, `max_shrink_ratio` and
    /// `cur_font_step` (-1 until a character sets them).
    max_stretch_ratio: i32,
    max_shrink_ratio: i32,
    cur_font_step: i32,
}

impl Breaker<'_> {
    /// `check_expand_pars(f)` (pdfTeX): whether `f` is expandable; the
    /// paragraph's step and limits are the first such font's.
    fn check_expand_pars(&mut self, f: FontId) -> bool {
        let Some(x) = self.fonts.expansion(f) else {
            return false;
        };
        if x.step == 0 || (x.stretch.is_none() && x.shrink.is_none()) {
            return false;
        }
        if self.cur_font_step < 0 {
            self.cur_font_step = x.step;
        }
        if let Some((_, r)) = x.stretch
            && self.max_stretch_ratio < 0
        {
            self.max_stretch_ratio = r;
        }
        if let Some((_, r)) = x.shrink
            && self.max_shrink_ratio < 0
        {
            self.max_shrink_ratio = -r;
        }
        true
    }

    /// A character's width with, where `\pdfadjustspacing>1` and its font
    /// is expandable, its stretch and shrink (`add_char_stretch`, ...);
    /// it becomes `prev_char_p`.
    fn char_widths(&mut self, n: &Node, at: Pos) -> (Scaled, Scaled, Scaled) {
        let Some((f, c)) = crate::microtype::char_of(n) else {
            return (0, 0, 0);
        };
        let w = self.fonts.char_width(f, c);
        if self.p.adjust_spacing > 1 && self.check_expand_pars(f) {
            self.prev_char = Some(at);
            return (
                w,
                crate::microtype::char_stretch(self.fonts, f, c),
                crate::microtype::char_shrink(self.fonts, f, c),
            );
        }
        (w, 0, 0)
    }

    /// A font kern's stretch and shrink (`kern_stretch`, `kern_shrink`)
    /// where `\pdfadjustspacing>1`.
    fn kern_widths(&self, list: &[Node], at: Pos, w: Scaled) -> (Scaled, Scaled) {
        if self.p.adjust_spacing <= 1 {
            return (0, 0);
        }
        let Some(pc) = self.prev_char else {
            return (0, 0);
        };
        if pos_next(list, pc) != Some(at) {
            return (0, 0);
        }
        let Some(nx) = pos_next(list, at) else {
            return (0, 0);
        };
        let l = pos_node(list, pc).and_then(crate::microtype::char_of);
        let r = pos_node(list, nx).and_then(crate::microtype::char_of);
        (
            crate::microtype::kern_fw(self.fonts, l, w, r, true),
            crate::microtype::kern_fw(self.fonts, l, w, r, false),
        )
    }

    /// The width of a node of a discretionary or its replacement, with
    /// its stretch and shrink (§841, §842, §870, §871 and pdfTeX's).
    fn disc_item_widths(&mut self, list: &[Node], n: &Node, at: Pos) -> (Scaled, Scaled, Scaled) {
        match n {
            Node::Char { .. } | Node::Lig(_) => self.char_widths(n, at),
            Node::Kern(w, crate::node::KernKind::Normal) => {
                let (st, sh) = self.kern_widths(list, at, *w);
                (*w, st, sh)
            }
            n => (disc_item_width(n, self.fonts), 0, 0),
        }
    }

    /// `total_pw(r, cur_p)` (pdfTeX): the protrusion of a line from the
    /// break of active node `passive` to `cur_p`, and its left and right
    /// characters (`last_leftmost_char`, `last_rightmost_char`).
    fn total_pw(
        &self,
        list: &[Node],
        passive: Option<usize>,
        cur_p: Option<usize>,
    ) -> (Scaled, Edge, Edge) {
        use crate::microtype::{char_pw, find_protchar_left, find_protchar_right};
        // (`l`: the break the line starts after; `first_p` at the start)
        let l: Option<usize> = match passive {
            None => (!list.is_empty()).then_some(0),
            Some(k) => self.passive[k].cur_break,
        };
        let right = match cur_p.map(|c| &list[c]) {
            Some(Node::Disc(d)) if !d.pre.is_empty() => d.pre.last(),
            _ => {
                let r = match cur_p {
                    Some(c) => c.checked_sub(1),
                    None => list.len().checked_sub(1),
                };
                match (l, r) {
                    (Some(l), Some(r)) => find_protchar_right(list, l, r),
                    _ => None,
                }
            }
        };
        let left = match l {
            Some(li) => match &list[li] {
                Node::Disc(d) if !d.post.is_empty() => d.post.first(),
                Node::Disc(_) => find_protchar_left(list, li + 1, true),
                _ => find_protchar_left(list, li, true),
            },
            None => None,
        };
        let lig = |n: Option<&Node>| matches!(n, Some(Node::Lig(_)));
        let (lw, lp) = char_pw(self.fonts, left, true);
        let (rw, rp) = char_pw(self.fonts, right, false);
        (
            lw + rw,
            lp.map(|(f, c)| (f, c, lig(left))),
            rp.map(|(f, c)| (f, c, lig(right))),
        )
    }

    /// §850 and §889: width and indent of line `l`.
    fn line_shape(&self, l: i32) -> (Scaled, Scaled) {
        if l > self.last_special_line {
            (self.second_width, self.second_indent)
        } else if self.p.par_shape.is_empty() {
            (self.first_width, self.first_indent)
        } else {
            let (i, w) = self.p.par_shape[usize::try_from(l - 1).unwrap_or(0)];
            (w, i)
        }
    }

    /// §837–§842: `break_width` at `cur_p`.
    fn break_width(&mut self, list: &List, cur_p: Option<usize>, hyphenated: bool) -> Widths {
        let mut bw = self.background;
        let Some(cp) = cur_p else { return bw };
        let mut s = Some(cp);
        if hyphenated && let Node::Disc(d) = &list[cp] {
            // §840
            for (j, v) in d.replace.iter().enumerate() {
                let (w, st, sh) = self.disc_item_widths(list, v, Pos::Rep(cp, j));
                bw[0] -= i64::from(w);
                bw[6] -= i64::from(st);
                bw[7] -= i64::from(sh);
            }
            for (j, v) in d.post.iter().enumerate() {
                let (w, st, sh) = self.disc_item_widths(list, v, Pos::Post(cp, j));
                bw[0] += i64::from(w);
                bw[6] += i64::from(st);
                bw[7] += i64::from(sh);
            }
            bw[0] += i64::from(self.disc_width);
            bw[6] += i64::from(self.disc_fw[0]);
            bw[7] += i64::from(self.disc_fw[1]);
            s = if d.post.is_empty() {
                Some(cp + 1)
            } else {
                None
            };
        }
        while let Some(i) = s {
            match list.get(i) {
                Some(Node::Glue(g)) => add_glue(&mut bw, &g.spec, -1),
                Some(Node::Penalty(_)) => {}
                Some(Node::Math(_, w) | Node::Kern(w, crate::node::KernKind::Explicit)) => {
                    bw[0] -= i64::from(*w);
                }
                _ => break,
            }
            s = Some(i + 1);
        }
        bw
    }

    /// `try_break(pi, break_type)` (§829).
    #[allow(clippy::too_many_lines)]
    fn try_break(&mut self, list: &List, cur_p: Option<usize>, mut pi: i32, hyphenated: bool) {
        // §831
        if pi.abs() >= INF_PENALTY {
            if pi > 0 {
                return;
            }
            pi = EJECT_PENALTY;
        }
        let mut no_break_yet = true;
        let mut break_width = self.background;
        let mut old_l = 0;
        let mut line_width: Scaled = 0;
        let mut i = 0;
        loop {
            let at_end = i == self.active.len();
            let l = if at_end {
                MAX_HALFWORD
            } else {
                self.active[i].line_number
            };
            // §835
            if l > old_l {
                if self.minimum_demerits < AWFUL_BAD && (old_l != self.easy_line || at_end) {
                    // §836
                    if no_break_yet {
                        no_break_yet = false;
                        break_width = self.break_width(list, cur_p, hyphenated);
                    }
                    let adj = self.p.adj_demerits.abs();
                    if adj >= AWFUL_BAD - self.minimum_demerits {
                        self.minimum_demerits = AWFUL_BAD - 1;
                    } else {
                        self.minimum_demerits += adj;
                    }
                    let mut alpha = self.total;
                    for k in 0..8 {
                        alpha[k] -= break_width[k];
                    }
                    for fit in VERY_LOOSE_FIT..=TIGHT_FIT {
                        if self.minimal_demerits[fit] <= self.minimum_demerits {
                            // §845
                            self.passive.push(Passive {
                                cur_break: cur_p,
                                prev_break: self.best_place[fit],
                            });
                            self.active.insert(
                                i,
                                Active {
                                    passive: Some(self.passive.len() - 1),
                                    line_number: self.best_pl_line[fit] + 1,
                                    fitness: fit,
                                    hyphenated,
                                    total_demerits: self.minimal_demerits[fit],
                                    alpha,
                                    short: self.best_pl_short[fit],
                                    glue: self.best_pl_glue[fit],
                                },
                            );
                            i += 1;
                        }
                        self.minimal_demerits[fit] = AWFUL_BAD;
                    }
                    self.minimum_demerits = AWFUL_BAD;
                }
                if at_end {
                    return;
                }
                // §850
                if l > self.easy_line {
                    line_width = self.second_width;
                    old_l = MAX_HALFWORD - 1;
                } else {
                    old_l = l;
                    line_width = self.line_shape(l).0;
                }
            }
            // §851
            let r = &self.active[i];
            let mut caw = [0i64; 8];
            for (k, c) in caw.iter_mut().enumerate() {
                *c = self.total[k] - r.alpha[k];
            }
            let caw: [Scaled; 8] = caw.map(to_scaled);
            let r_passive = r.passive;
            let mut artificial_demerits = false;
            let mut shortfall = line_width - caw[0];
            // pdfTeX: the line's protrusion and its fonts' expansion
            let mut edges = (None, None);
            if self.p.protrude_chars > 1 {
                let (pw, lp, rp) = self.total_pw(list, r_passive, cur_p);
                shortfall += pw;
                edges = (lp, rp);
            }
            if self.p.adjust_spacing > 1 && shortfall != 0 {
                let (mut mks, mut mksh) = (0, 0);
                if self.p.protrude_chars > 1 {
                    // "Calculate variations of marginal kerns": `left_pw`
                    // of both edges, and of a ligature's edge 0 (pdfTeX
                    // asks `char_pw` of its `lig_char`, no char node)
                    use crate::microtype::{pw_of, subst_font};
                    for (f, c, lig) in [edges.0, edges.1].into_iter().flatten() {
                        let own = if lig { 0 } else { pw_of(self.fonts, f, c, true) };
                        let k = subst_font(self.fonts, f, c, 1000);
                        if k != f {
                            mks += own - pw_of(self.fonts, k, c, true);
                        }
                        let k = subst_font(self.fonts, f, c, -1000);
                        if k != f {
                            mksh += pw_of(self.fonts, k, c, true) - own;
                        }
                    }
                }
                let (fs, fsh) = (caw[6] + mks, caw[7] + mksh);
                if shortfall > 0 && fs > 0 {
                    shortfall = if fs > shortfall {
                        (fs / (self.max_stretch_ratio / self.cur_font_step).max(1)) / 2
                    } else {
                        shortfall - fs
                    };
                } else if shortfall < 0 && fsh > 0 {
                    shortfall = if fsh > -shortfall {
                        -((fsh / (self.max_shrink_ratio / self.cur_font_step).max(1)) / 2)
                    } else {
                        shortfall + fsh
                    };
                }
            }
            let mut g: Scaled = 0;
            let mut b: i32;
            let mut fit_class: usize;
            let mut found = false;
            if shortfall > 0 {
                // §852
                if caw[2] != 0 || caw[3] != 0 || caw[4] != 0 {
                    b = 0;
                    fit_class = DECENT_FIT;
                    if self.do_last_line_fit {
                        if cur_p.is_none()
                            && let Some((bb, fc, gg)) = self.last_line(r, &caw, shortfall)
                        {
                            b = bb;
                            fit_class = fc;
                            g = gg;
                            found = true;
                        }
                        if !found {
                            shortfall = 0;
                        }
                    }
                } else if shortfall > 7_230_584 && caw[1] < 1_663_497 {
                    b = INF_BAD;
                    fit_class = VERY_LOOSE_FIT;
                } else {
                    b = badness(shortfall, caw[1]);
                    fit_class = if b > 12 {
                        if b > 99 { VERY_LOOSE_FIT } else { LOOSE_FIT }
                    } else {
                        DECENT_FIT
                    };
                }
            } else {
                // §853
                b = if -shortfall > caw[5] {
                    INF_BAD + 1
                } else {
                    badness(-shortfall, caw[5])
                };
                fit_class = if b > 12 { TIGHT_FIT } else { DECENT_FIT };
            }
            if self.do_last_line_fit && !found {
                // "Adjust the additional data for last line"
                if cur_p.is_none() {
                    shortfall = 0;
                }
                g = match shortfall.cmp(&0) {
                    std::cmp::Ordering::Greater => caw[1],
                    std::cmp::Ordering::Less => caw[5],
                    std::cmp::Ordering::Equal => 0,
                };
            }
            // found:

            let node_r_stays_active = if b > INF_BAD || pi == EJECT_PENALTY {
                // §854
                if self.final_pass
                    && self.minimum_demerits == AWFUL_BAD
                    && i == 0
                    && self.active.len() == 1
                {
                    artificial_demerits = true;
                } else if b > self.threshold {
                    self.active.remove(i);
                    continue;
                }
                false
            } else {
                if b > self.threshold {
                    i += 1;
                    continue;
                }
                true
            };
            // §855
            let r = &self.active[i];
            let mut d = if artificial_demerits {
                0
            } else {
                // §859
                let mut d = self.p.line_penalty.wrapping_add(b);
                d = if d.abs() >= 10000 { 100_000_000 } else { d * d };
                if pi != 0 {
                    if pi > 0 {
                        d = d.wrapping_add(pi * pi);
                    } else if pi > EJECT_PENALTY {
                        d = d.wrapping_sub(pi * pi);
                    }
                }
                if hyphenated && r.hyphenated {
                    d = d.wrapping_add(if cur_p.is_some() {
                        self.p.double_hyphen_demerits
                    } else {
                        self.p.final_hyphen_demerits
                    });
                }
                if fit_class.abs_diff(r.fitness) > 1 {
                    d = d.wrapping_add(self.p.adj_demerits);
                }
                d
            };
            d = d.wrapping_add(r.total_demerits);
            if d <= self.minimal_demerits[fit_class] {
                self.minimal_demerits[fit_class] = d;
                self.best_place[fit_class] = r.passive;
                self.best_pl_line[fit_class] = l;
                if self.do_last_line_fit {
                    self.best_pl_short[fit_class] = shortfall;
                    self.best_pl_glue[fit_class] = g;
                }
                if d < self.minimum_demerits {
                    self.minimum_demerits = d;
                }
            }
            if node_r_stays_active {
                i += 1;
            } else {
                // §860
                self.active.remove(i);
            }
        }
    }

    /// e-TeX: "Perform computations for last line and goto found";
    /// `None` falls through to the normal algorithm.
    fn last_line(
        &self,
        r: &Active,
        caw: &[Scaled; 8],
        shortfall: Scaled,
    ) -> Option<(i32, usize, Scaled)> {
        if r.short == 0 || r.glue <= 0 {
            return None;
        }
        if caw[2] != self.fill_width[0]
            || caw[3] != self.fill_width[1]
            || caw[4] != self.fill_width[2]
        {
            return None;
        }
        let mut g = if r.short > 0 { caw[1] } else { caw[5] };
        if g <= 0 {
            return None;
        }
        let mut err = false;
        g = fract(g, r.short, r.glue, MAX_DIMEN).unwrap_or_else(|| {
            err = true;
            0
        });
        if self.p.last_line_fit < 1000 {
            g = fract(g, self.p.last_line_fit, 1000, MAX_DIMEN).unwrap_or_else(|| {
                err = true;
                0
            });
        }
        if err {
            g = if r.short > 0 { MAX_DIMEN } else { -MAX_DIMEN };
        }
        match g.cmp(&0) {
            std::cmp::Ordering::Greater => {
                if g > shortfall {
                    g = shortfall;
                }
                if g > 7_230_584 && caw[1] < 1_663_497 {
                    return Some((INF_BAD, VERY_LOOSE_FIT, g));
                }
                let b = badness(g, caw[1]);
                let fc = if b > 12 {
                    if b > 99 { VERY_LOOSE_FIT } else { LOOSE_FIT }
                } else {
                    DECENT_FIT
                };
                Some((b, fc, g))
            }
            std::cmp::Ordering::Less => {
                if -g > caw[5] {
                    g = -caw[5];
                }
                let b = badness(-g, caw[5]);
                Some((b, if b > 12 { TIGHT_FIT } else { DECENT_FIT }, g))
            }
            std::cmp::Ordering::Equal => None,
        }
    }

    fn add_width(&mut self, w: Scaled) {
        self.total[0] += i64::from(w);
    }

    fn kern_break(&mut self, list: &List, at: usize, auto_breaking: bool, w: Scaled) {
        // §866 kern_break
        if auto_breaking && matches!(list.get(at + 1), Some(Node::Glue(_))) {
            self.try_break(list, Some(at), 0, false);
        }
        self.add_width(w);
    }
}

/// `line_break(d)` (§815, e-TeX's form): `display` is true for the part
/// of a paragraph before display math (e-TeX's `d`). `list` is the
/// horizontal list as tex.web finds it at `link(head)`.
///
/// # Panics
/// On material that cannot be in a paragraph (tex.web's `confusion`).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn line_break(
    mut list: List,
    display: bool,
    params: &LineBreakParams,
    fonts: &dyn Fonts,
    hyphenator: &mut dyn Hyphenator,
) -> Paragraph {
    // §816
    if let Some(Node::Glue(_)) = list.last() {
        list.pop();
    }
    list.push(Node::Penalty(INF_PENALTY));
    list.push(Node::param_glue(param::PAR_FILL_SKIP, params.par_fill_skip));
    let last_line_fill = list.len() - 1;
    // §827: check_shrinkage of \leftskip and \rightskip
    let mut infinite_shrink_error = false;
    let finite = |g: &Glue, err: &mut bool| -> Glue {
        if g.shrink_order != Order::Normal && g.shrink != 0 {
            *err = true;
            Glue {
                shrink_order: Order::Normal,
                ..*g
            }
        } else {
            *g
        }
    };
    let left_skip = finite(&params.left_skip, &mut infinite_shrink_error);
    let right_skip = finite(&params.right_skip, &mut infinite_shrink_error);
    let mut background = [0i64; 8];
    add_glue(&mut background, &left_skip, 1);
    add_glue(&mut background, &right_skip, 1);
    let mut pool = POOL.take().unwrap_or_default();
    let mut b = Breaker {
        p: params,
        fonts,
        background,
        total: [0; 8],
        active: std::mem::take(&mut pool.active),
        passive: std::mem::take(&mut pool.passive),
        minimal_demerits: [AWFUL_BAD; 4],
        minimum_demerits: AWFUL_BAD,
        best_place: [None; 4],
        best_pl_line: [0; 4],
        best_pl_short: [0; 4],
        best_pl_glue: [0; 4],
        threshold: 0,
        second_pass: false,
        final_pass: false,
        disc_width: 0,
        easy_line: 0,
        last_special_line: 0,
        first_width: 0,
        second_width: 0,
        first_indent: 0,
        second_indent: 0,
        do_last_line_fit: false,
        fill_width: [0; 3],
        disc_fw: [0; 2],
        prev_char: None,
        max_stretch_ratio: -1,
        max_shrink_ratio: -1,
        cur_font_step: -1,
    };
    // e-TeX: "Check for special treatment of last line of paragraph"
    if params.last_line_fit > 0 {
        let q = &params.par_fill_skip;
        if q.stretch > 0
            && q.stretch_order > Order::Normal
            && background[2] == 0
            && background[3] == 0
            && background[4] == 0
        {
            b.do_last_line_fit = true;
            b.fill_width[q.stretch_order as usize - 1] = q.stretch;
        }
    }
    // §848, §849
    if params.par_shape.is_empty() {
        if params.hang_indent == 0 {
            b.last_special_line = 0;
            b.second_width = params.hsize;
            b.second_indent = 0;
        } else {
            b.last_special_line = params.hang_after.abs();
            let narrow = params.hsize - params.hang_indent.abs();
            let indent = params.hang_indent.max(0);
            if params.hang_after < 0 {
                b.first_width = narrow;
                b.first_indent = indent;
                b.second_width = params.hsize;
                b.second_indent = 0;
            } else {
                b.first_width = params.hsize;
                b.first_indent = 0;
                b.second_width = narrow;
                b.second_indent = indent;
            }
        }
    } else {
        let n = params.par_shape.len();
        b.last_special_line = i32::try_from(n).unwrap_or(i32::MAX) - 1;
        b.second_width = params.par_shape[n - 1].1;
        b.second_indent = params.par_shape[n - 1].0;
    }
    b.easy_line = if params.looseness == 0 {
        b.last_special_line
    } else {
        MAX_HALFWORD
    };

    // §863
    b.threshold = params.pretolerance;
    let mut pass = 1;
    if b.threshold < 0 {
        b.threshold = params.tolerance;
        b.second_pass = true;
        b.final_pass = params.emergency_stretch <= 0;
        pass = 2;
    }
    let mut hyphenated = false;
    let best_bet = loop {
        if b.threshold > INF_BAD {
            b.threshold = INF_BAD;
        }
        if b.second_pass && !hyphenated {
            list = hyphenate_all(
                list,
                params.lang,
                hyphenator,
                fonts,
                &mut pool.edits,
                &mut pool.unhyphenated,
            );
            hyphenated = true;
        }
        // §864
        b.active.clear();
        b.passive.clear();
        b.total = [0; 8];
        let alpha = b.background.map(|x| -x);
        b.active.push(Active {
            passive: None,
            line_number: params.prev_graf + 1,
            fitness: DECENT_FIT,
            hyphenated: false,
            total_demerits: 0,
            alpha,
            short: 0,
            glue: 0,
        });
        let mut auto_breaking = true;
        let mut cur = 0;
        let mut prev_p = 0;
        // (the font of the run of characters being measured, and its
        // table, [`Fonts::whd_table`])
        let mut run: Option<(u32, Option<&[crate::fonts::Whd]>)> = None;
        while cur < list.len() && !b.active.is_empty() {
            // §866
            match &list[cur] {
                Node::Char { font, ch } if params.adjust_spacing <= 1 => {
                    let table = match run {
                        Some((f, t)) if f == *font => t,
                        _ => {
                            let t = fonts.whd_table(*font);
                            run = Some((*font, t));
                            t
                        }
                    };
                    b.add_width(match table {
                        Some(t) => t.get(usize::from(*ch)).map_or(0, |x| x.0),
                        None => fonts.char_width(*font, *ch),
                    });
                }
                Node::Char { .. } | Node::Lig(_) => {
                    let (w, st, sh) = b.char_widths(&list[cur], Pos::Main(cur));
                    b.add_width(w);
                    b.total[6] += i64::from(st);
                    b.total[7] += i64::from(sh);
                }
                Node::Box(_) | Node::Rule(_) => {
                    b.add_width(list[cur].box_dims().map_or(0, |d| d.0));
                }
                Node::Whatsit(_, size) => {
                    // pdfTeX: "Advance past a whatsit node in the
                    // line_break loop"
                    b.add_width(size.map_or(0, |s| s.width));
                }
                // (§1362: a language node goes to the hyphenation, done
                // before the pass, [`hyphenate_all`])
                Node::Mark(_) | Node::Ins(_) | Node::Adjust(_) | Node::Language { .. } => {}
                Node::Glue(_) => {
                    // §868
                    if auto_breaking {
                        let pp = &list[prev_p];
                        if prev_p != cur
                            && (pp.precedes_break()
                                || matches!(pp, Node::Kern(_, k) if *k != crate::node::KernKind::Explicit))
                        {
                            b.try_break(&list, Some(cur), 0, false);
                        }
                    }
                    if let Node::Glue(g) = &mut list[cur] {
                        if g.spec.shrink_order != Order::Normal && g.spec.shrink != 0 {
                            infinite_shrink_error = true;
                            g.spec.shrink_order = Order::Normal;
                        }
                        let spec = g.spec;
                        add_glue(&mut b.total, &spec, 1);
                    }
                }
                Node::Kern(w, k) => {
                    let w = *w;
                    if *k == crate::node::KernKind::Explicit {
                        b.kern_break(&list, cur, auto_breaking, w);
                    } else {
                        b.add_width(w);
                        if *k == crate::node::KernKind::Normal {
                            let (st, sh) = b.kern_widths(&list, Pos::Main(cur), w);
                            b.total[6] += i64::from(st);
                            b.total[7] += i64::from(sh);
                        }
                    }
                }
                Node::Disc(d) => {
                    // §869
                    let mut dw = [0i64; 3];
                    for (j, s) in d.pre.iter().enumerate() {
                        let (w, st, sh) = b.disc_item_widths(&list, s, Pos::Pre(cur, j));
                        dw[0] += i64::from(w);
                        dw[1] += i64::from(st);
                        dw[2] += i64::from(sh);
                    }
                    b.disc_width = to_scaled(dw[0]);
                    b.disc_fw = [to_scaled(dw[1]), to_scaled(dw[2])];
                    if d.pre.is_empty() {
                        b.try_break(&list, Some(cur), params.ex_hyphen_penalty, true);
                    } else {
                        for (k, x) in [(0, dw[0]), (6, dw[1]), (7, dw[2])] {
                            b.total[k] += x;
                        }
                        b.try_break(&list, Some(cur), params.hyphen_penalty, true);
                        for (k, x) in [(0, dw[0]), (6, dw[1]), (7, dw[2])] {
                            b.total[k] -= x;
                        }
                    }
                    if let Node::Disc(d) = &list[cur] {
                        for (j, s) in d.replace.iter().enumerate() {
                            let (w, st, sh) = b.disc_item_widths(&list, s, Pos::Rep(cur, j));
                            b.add_width(w);
                            b.total[6] += i64::from(st);
                            b.total[7] += i64::from(sh);
                        }
                    }
                }
                Node::Math(kind, w) => {
                    let w = *w;
                    auto_breaking = *kind == crate::node::MathKind::After;
                    b.kern_break(&list, cur, auto_breaking, w);
                }
                Node::Penalty(pen) => {
                    let pen = *pen;
                    b.try_break(&list, Some(cur), pen, false);
                }
                Node::Unset(_) => panic!("line_break: unset node in a paragraph"),
            }
            prev_p = cur;
            cur += 1;
        }
        if cur == list.len() {
            // §873
            b.try_break(&list, None, EJECT_PENALTY, true);
            if !b.active.is_empty() {
                // §874
                let mut best = 0;
                let mut fewest = AWFUL_BAD;
                for (k, r) in b.active.iter().enumerate() {
                    if r.total_demerits < fewest {
                        fewest = r.total_demerits;
                        best = k;
                    }
                }
                if params.looseness == 0 {
                    break best;
                }
                // §875
                let best_line = b.active[best].line_number;
                let mut actual_looseness = 0;
                for (k, r) in b.active.iter().enumerate() {
                    let line_diff = r.line_number - best_line;
                    if (line_diff < actual_looseness && params.looseness <= line_diff)
                        || (line_diff > actual_looseness && params.looseness >= line_diff)
                    {
                        best = k;
                        actual_looseness = line_diff;
                        fewest = r.total_demerits;
                    } else if line_diff == actual_looseness && r.total_demerits < fewest {
                        best = k;
                        fewest = r.total_demerits;
                    }
                }
                if actual_looseness == params.looseness || b.final_pass {
                    break best;
                }
            }
        }
        if b.second_pass {
            b.background[1] += i64::from(params.emergency_stretch);
            b.final_pass = true;
            pass = 3;
        } else {
            b.threshold = params.tolerance;
            b.second_pass = true;
            b.final_pass = params.emergency_stretch <= 0;
            pass = 2;
        }
    };
    let best = b.active[best_bet].clone();
    // e-TeX: "Adjust the final line of the paragraph" (`\parfillskip`,
    // the list's last node)
    let _ = last_line_fill;
    let last_line_fill = list.len() - 1;
    if b.do_last_line_fit
        && best.short != 0
        && let Node::Glue(g) = &mut list[last_line_fill]
    {
        g.spec.width += best.short - best.glue;
        g.spec.stretch = 0;
    }
    // §878: the breaks, first to last
    let mut breaks = std::mem::take(&mut pool.breaks);
    breaks.clear();
    let mut q = best.passive;
    while let Some(k) = q {
        breaks.push(b.passive[k].cur_break);
        q = b.passive[k].prev_break;
    }
    breaks.reverse();
    let lines = post_line_break(
        list,
        &breaks,
        &mut pool.lines,
        best.line_number,
        display,
        params,
        &b,
        left_skip,
        right_skip,
    );
    pool.breaks = breaks;
    b.active.clear();
    b.passive.clear();
    pool.active = b.active;
    pool.passive = b.passive;
    POOL.set(Some(pool));
    Paragraph {
        lines,
        prev_graf: best.line_number - 1,
        infinite_shrink_error,
        pass,
    }
}

/// `post_line_break(d)` (§877–§890).
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
fn post_line_break(
    list: List,
    breaks: &[Option<usize>],
    spare: &mut Vec<List>,
    best_line: i32,
    display: bool,
    params: &LineBreakParams,
    b: &Breaker,
    left_skip: Glue,
    right_skip: Glue,
) -> Vec<Line> {
    // (the nodes not yet placed: `front`, a discretionary's post-break
    // material still to come, last first, then the list from `it`, whose
    // next node is the list's `pos`; a line's run of the list's nodes
    // is moved in one go)
    let mut it = list.into_iter();
    let mut pos = 0usize;
    let mut front: Vec<Node> = Vec::new();
    let mut lines = Vec::with_capacity(breaks.len());
    let mut last_at = 0usize;
    for (cur_line, (k, &brk)) in (params.prev_graf + 1..).zip(breaks.iter().enumerate()) {
        // §881
        let mut line = spare.pop().unwrap_or_default();
        line.reserve(match brk {
            Some(at) => at.saturating_sub(last_at) + front.len() + 4,
            None => it.len() + front.len() + 2,
        });
        let mut disc_break = false;
        let mut post_disc_break = false;
        let protrude = params.protrude_chars > 0;
        match brk {
            None => {
                line.extend(front.drain(..).rev());
                line.extend(it.by_ref());
                if protrude && let Some(q) = line.len().checked_sub(1) {
                    right_margin(&mut line, q, false, b.fonts);
                }
                line.push(Node::param_glue(param::RIGHT_SKIP, right_skip));
            }
            Some(at) => {
                last_at = at;
                line.extend(front.drain(..).rev());
                let n = at.saturating_sub(pos);
                let before = it.len();
                line.extend(it.by_ref().take(n));
                pos += before - it.len();
                // (the break node, where the list reaches it)
                if pos == at
                    && let Some(n) = it.next()
                {
                    pos += 1;
                    // (pdfTeX: the right margin's kern after the node
                    // before the break, `q`, or after the pre-break text)
                    match n {
                        Node::Glue(_) => {
                            if protrude {
                                let q = line.len();
                                right_margin(&mut line, q, false, b.fonts);
                            }
                            line.push(Node::param_glue(param::RIGHT_SKIP, right_skip));
                        }
                        Node::Disc(d) => {
                            // §882–§885
                            line.push(Node::Disc(Box::default()));
                            let q = line.len() - 1;
                            if !d.post.is_empty() {
                                post_disc_break = true;
                                front.extend(d.post.into_iter().rev());
                            }
                            let pre = !d.pre.is_empty();
                            line.extend(d.pre);
                            disc_break = true;
                            if protrude {
                                if pre {
                                    let q = line.len() - 1;
                                    right_margin(&mut line, q, true, b.fonts);
                                } else {
                                    right_margin(&mut line, q, false, b.fonts);
                                }
                            }
                            line.push(Node::param_glue(param::RIGHT_SKIP, right_skip));
                        }
                        n => {
                            line.push(match n {
                                Node::Math(kind, _) => Node::Math(kind, 0),
                                Node::Kern(_, kind) => Node::Kern(0, kind),
                                n => n,
                            });
                            if protrude {
                                let q = line.len() - 1;
                                right_margin(&mut line, q, false, b.fonts);
                            }
                            line.push(Node::param_glue(param::RIGHT_SKIP, right_skip));
                        }
                    }
                } else {
                    pos += it.len();
                    line.extend(it.by_ref());
                }
            }
        }
        // pdfTeX: the left margin's kern
        if protrude {
            let p = crate::microtype::find_protchar_left(&line, 0, false);
            let (w, ch) = crate::microtype::char_pw(b.fonts, p, true);
            if w != 0
                && let Some((font, ch)) = ch
            {
                line.insert(
                    0,
                    Node::Kern(
                        -w,
                        crate::node::KernKind::Margin {
                            left: true,
                            font,
                            ch,
                        },
                    ),
                );
            }
        }
        // §887: trap_zero_glue makes a zero \leftskip the zero_glue
        if left_skip.width != 0 || left_skip.stretch != 0 || left_skip.shrink != 0 {
            line.insert(0, Node::param_glue(param::LEFT_SKIP, left_skip));
        }
        // §889
        let (cur_width, cur_indent) = b.line_shape(cur_line);
        let mut packed = if params.adjust_spacing > 0 {
            crate::microtype::hpack_expand(line, cur_width, b.fonts, &params.pack, true)
        } else {
            hpack(line, Spec::Exactly(cur_width), b.fonts, &params.pack, true)
        };
        packed.node.shift = cur_indent;
        // §890
        let mut penalty = None;
        if cur_line + 1 != best_line {
            let pick = |arr: &[i32], r: i32| -> i32 {
                let n = i32::try_from(arr.len()).unwrap_or(i32::MAX);
                arr[usize::try_from(r.min(n) - 1).unwrap_or(0)]
            };
            let mut pen = if params.inter_line_penalties.is_empty() {
                params.inter_line_penalty
            } else {
                pick(&params.inter_line_penalties, cur_line)
            };
            if !params.club_penalties.is_empty() {
                pen += pick(&params.club_penalties, cur_line - params.prev_graf);
            } else if cur_line == params.prev_graf + 1 {
                pen += params.club_penalty;
            }
            let widows = if display {
                &params.display_widow_penalties
            } else {
                &params.widow_penalties
            };
            if !widows.is_empty() {
                pen += pick(widows, best_line - cur_line - 1);
            } else if cur_line + 2 == best_line {
                pen += if display {
                    params.display_widow_penalty
                } else {
                    params.widow_penalty
                };
            }
            if disc_break {
                pen += params.broken_penalty;
            }
            if pen != 0 {
                penalty = Some(pen);
            }
        }
        lines.push(Line { packed, penalty });
        // §879
        if let Some(&next) = breaks.get(k + 1)
            && !post_disc_break
        {
            loop {
                let (idx, n) = if let Some(n) = front.last() {
                    (None, n)
                } else if let Some(n) = it.as_slice().first() {
                    (Some(pos), n)
                } else {
                    break;
                };
                if next.is_some() && idx == next {
                    break;
                }
                let prune = match n {
                    Node::Kern(_, kind) => *kind == crate::node::KernKind::Explicit,
                    n => !n.precedes_break(),
                };
                if !prune {
                    break;
                }
                if front.pop().is_none() {
                    it.next();
                    pos += 1;
                }
            }
        }
    }
    lines
}

/// pdfTeX: the right margin's kern of a line, after the node before
/// `line[q]` (the break, or the line's last node) or, `pre`, after
/// `line[q]` itself (a discretionary's pre-break text's last node).
fn right_margin(line: &mut List, q: usize, pre: bool, fonts: &dyn Fonts) {
    let ptmp = if pre { Some(q) } else { q.checked_sub(1) };
    let Some(pt) = ptmp else { return };
    let p = if pre {
        line.get(pt)
    } else {
        crate::microtype::find_protchar_right(line, 0, pt)
    };
    let (w, ch) = crate::microtype::char_pw(fonts, p, false);
    if w != 0
        && let Some((font, ch)) = ch
    {
        line.insert(
            pt + 1,
            Node::Kern(
                -w,
                crate::node::KernKind::Margin {
                    left: false,
                    font,
                    ch,
                },
            ),
        );
    }
}

/// `append_to_vlist(b)` (§679): the interline glue to put before a box of
/// height `height` when the previous depth is `prev_depth`, or `None`
/// when `prev_depth <= ignore_depth` (-1000pt).
#[must_use]
pub fn interline_glue(
    prev_depth: Scaled,
    height: Scaled,
    baseline_skip: Glue,
    line_skip: Glue,
    line_skip_limit: Scaled,
) -> Option<Node> {
    if prev_depth <= -65_536_000 {
        return None;
    }
    let d = baseline_skip.width - prev_depth - height;
    Some(if d < line_skip_limit {
        Node::param_glue(param::LINE_SKIP, line_skip)
    } else {
        Node::param_glue(
            param::BASELINE_SKIP,
            Glue {
                width: d,
                ..baseline_skip
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{MathKind, Rule};
    use crate::testfont::TestFonts;

    struct Count(Vec<usize>);
    impl Hyphenator for Count {
        fn hyphenate(
            &mut self,
            _: &[Node],
            at: usize,
            _: LangState,
            _: &dyn Fonts,
        ) -> Option<(usize, usize, List)> {
            self.0.push(at);
            None
        }
    }

    fn rule(w: Scaled) -> Node {
        Node::Rule(Rule {
            width: w,
            height: 0,
            depth: 0,
        })
    }

    #[test]
    fn hyphenation_hook_on_second_pass_outside_math() {
        let sp = Node::glue(Glue::new(0x1_0000, 0x1_0000, 0));
        let list = vec![
            rule(0x10_0000),
            sp.clone(),
            Node::Math(MathKind::Before, 0),
            sp.clone(),
            Node::Math(MathKind::After, 0),
            sp.clone(),
            rule(0x10_0000),
        ];
        let p = LineBreakParams {
            pretolerance: -1,
            hsize: 0x30_0000,
            ..LineBreakParams::default()
        };
        let mut h = Count(Vec::new());
        let para = line_break(list, false, &p, &TestFonts::default(), &mut h);
        assert_eq!(para.pass, 2);
        assert_eq!(para.lines.len(), 1);
        // the glue inside math is not offered
        assert_eq!(h.0, vec![1, 5, 8]); // not inside math; \parfillskip too
    }

    #[test]
    fn interline_glue_rules() {
        let bs = Glue::new(12 * 0x1_0000, 0, 0);
        let ls = Glue::new(0x1_0000, 0, 0);
        assert_eq!(interline_glue(-65_536_000, 0, bs, ls, 0), None);
        let Some(Node::Glue(g)) = interline_glue(2 * 0x1_0000, 7 * 0x1_0000, bs, ls, 0) else {
            panic!()
        };
        assert_eq!(g.spec.width, 3 * 0x1_0000);
        let Some(Node::Glue(g)) = interline_glue(2 * 0x1_0000, 11 * 0x1_0000, bs, ls, 0) else {
            panic!()
        };
        assert_eq!(
            (g.kind, g.spec),
            (crate::node::GlueKind::Param(param::LINE_SKIP), ls)
        );
    }
}
