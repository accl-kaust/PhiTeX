//! C3: line breaking as a pure function (tex.web 813–890, e-TeX
//! `\lastlinefit`, pdfTeX protrusion/expansion); hyphenation via `hyph.rs`.
//!
//! `line_break` takes the paragraph's horizontal list and everything it reads
//! (`LineParams`, gathered by `list::end_graf`) and gives the lines: boxes with
//! interline glue and penalties between them, with the material the packager
//! migrated out of each line (inserts, marks, `\vadjust`) after its box.
//! Nothing else is read or changed: no state outlives the call.
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::diag::Diag;
use crate::font::{Font, Fonts, LigKern};
use crate::hyph::{Exceptions, Patterns};
use crate::item::{BoxNode, Disc, Item, Whatsit};
use crate::pack;
use crate::prim::Registry;
use crate::val::{FontId, Glue};

pub fn register(_r: &mut Registry) {}

const INF_BAD: i32 = 10000;
const INF_PENALTY: i32 = 10000;
const EJECT_PENALTY: i32 = -10000;
const AWFUL_BAD: i32 = 0o7777777777;
const MAX_HALFWORD: i32 = 0x0FFF_FFFF;
const MAX_DIMEN: i32 = 0o7777777777;
/// `prev_depth` value that suppresses interline glue (tex.web 212).
pub const IGNORE_DEPTH: i32 = -65_536_000;
/// `non_char` in hyphenation arrays.
const NON: u16 = 256;

const VERY_LOOSE: u8 = 0;
const LOOSE: u8 = 1;
const DECENT: u8 = 2;
const TIGHT: u8 = 3;

/// Glue subtypes: glue parameter code + 1 (tex.web 224).
const SUB_LINESKIP: u8 = 1;
const SUB_BASELINESKIP: u8 = 2;
const SUB_LEFTSKIP: u8 = 8;
const SUB_RIGHTSKIP: u8 = 9;
const SUB_PARFILLSKIP: u8 = 15;

/// e-TeX math node subtype `L_code`: subtypes below it are TeX's before/after.
const L_CODE: u8 = 4;

/// Everything line breaking reads (gathered by `list::end_graf`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LineParams {
    pub pretolerance: i32,
    pub tolerance: i32,
    pub emergency_stretch: i32,
    pub looseness: i32,
    pub line_penalty: i32,
    pub hyphen_penalty: i32,
    pub ex_hyphen_penalty: i32,
    pub adj_demerits: i32,
    pub double_hyphen_demerits: i32,
    pub final_hyphen_demerits: i32,
    pub hsize: i32,
    /// `\parshape` as (indent, width) per line; empty: none.
    pub parshape: Arc<[(i32, i32)]>,
    pub hang_indent: i32,
    pub hang_after: i32,
    pub left_skip: Glue,
    pub right_skip: Glue,
    pub par_fill_skip: Glue,
    pub inter_line_penalty: i32,
    pub club_penalty: i32,
    pub widow_penalty: i32,
    pub display_widow_penalty: i32,
    pub broken_penalty: i32,
    /// e-TeX `\interlinepenalties` … `\displaywidowpenalties`: the n values
    /// (without the count); empty: not set.
    pub inter_line_penalties: Arc<[i32]>,
    pub club_penalties: Arc<[i32]>,
    pub widow_penalties: Arc<[i32]>,
    pub display_widow_penalties: Arc<[i32]>,
    /// The paragraph is broken off by a display (`line_break(true)`).
    pub display: bool,
    pub baseline_skip: Glue,
    pub line_skip: Glue,
    pub line_skip_limit: i32,
    /// `\prevdepth` of the enclosing vertical list before the first line.
    pub prev_depth: i32,
    /// `\prevgraf` before the paragraph (0, or lines above a display).
    pub prev_graf: i32,
    /// e-TeX `\lastlinefit`.
    pub last_line_fit: i32,
    /// Language, `\lefthyphenmin`, `\righthyphenmin` at the paragraph start (normalized).
    pub lang: i32,
    pub lhm: i32,
    pub rhm: i32,
    pub uc_hyph: i32,
    /// Patterns and exceptions of the languages the paragraph uses.
    pub langs: BTreeMap<u8, (Option<Patterns>, Option<Exceptions>)>,
    /// `\lccode`s of the characters in the paragraph (absent: 0).
    pub lc: BTreeMap<u16, u16>,
    /// `\hyphenchar` of the fonts in the paragraph (absent: -1).
    pub hyphen_chars: BTreeMap<FontId, i32>,
    /// pdfTeX `\pdfprotrudechars`, `\pdfadjustspacing`.
    pub protrude_chars: i32,
    pub adjust_spacing: i32,
    /// pdfTeX `\lpcode` / `\rpcode` of (font, char) (absent: 0).
    pub lp_codes: BTreeMap<(FontId, u16), i32>,
    pub rp_codes: BTreeMap<(FontId, u16), i32>,
    /// What `hpack` reads for the lines (`\hbadness`, `\hfuzz`, …).
    pub pack: pack::Params,
}

/// The lines (boxes and the vertical material between them) and `\prevgraf`.
pub struct Lines {
    pub items: Vec<Item>,
    pub prev_graf: i32,
    pub last: Option<Arc<BoxNode>>,
    /// Errors and warnings (infinite shrinkage, under/overfull lines), in order.
    pub diags: Vec<Arc<Diag>>,
}

/// `append_to_vlist` (tex.web 679): interline glue before `b` from
/// `prev_depth`, then `b`; `prev_depth` becomes `b`'s depth.
pub fn append_to_vlist(
    out: &mut Vec<Item>,
    prev_depth: &mut i32,
    b: Arc<BoxNode>,
    baseline_skip: Glue,
    line_skip: Glue,
    line_skip_limit: i32,
) {
    if *prev_depth > IGNORE_DEPTH {
        let d = baseline_skip.width - *prev_depth - b.height;
        if d < line_skip_limit {
            out.push(Item::Glue { g: line_skip, sub: SUB_LINESKIP, leader: None });
        } else {
            let mut g = baseline_skip;
            g.width = d;
            out.push(Item::Glue { g, sub: SUB_BASELINESKIP, leader: None });
        }
    }
    *prev_depth = b.depth;
    out.push(Item::Box(b));
}

/// tex.web 108.
fn badness(t: i32, s: i32) -> i32 {
    if t == 0 {
        return 0;
    }
    if s <= 0 {
        return INF_BAD;
    }
    let r: i64 = if t <= 7_230_584 {
        (t as i64 * 297) / s as i64
    } else if s >= 1_663_497 {
        t as i64 / (s as i64 / 297)
    } else {
        t as i64
    };
    if r > 1290 { INF_BAD } else { ((r * r * r + 0o400000) / 0o1000000) as i32 }
}

/// e-TeX `fract`: x·n/d rounded, `None` (arith_error) beyond `max`.
fn fract(x: i32, n: i32, d: i32, max: i32) -> Option<i32> {
    if d == 0 {
        return None;
    }
    let mut positive = true;
    let (mut x, mut n, mut d) = (x as i128, n as i128, d as i128);
    if d < 0 {
        d = -d;
        positive = !positive;
    }
    if x < 0 {
        x = -x;
        positive = !positive;
    } else if x == 0 {
        return Some(0);
    }
    if n < 0 {
        n = -n;
        positive = !positive;
    }
    let a = (2 * x * n + d) / (2 * d);
    if a > max as i128 {
        return None;
    }
    let a = a as i32;
    Some(if positive { a } else { -a })
}

/// pdfTeX `round_xn_over_d`.
fn round_xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let pos = (x >= 0) == (n >= 0);
    let a = (x as i64).abs() * (n as i64).abs();
    let d = d as i64;
    let mut q = a / d;
    if 2 * (a % d) >= d {
        q += 1;
    }
    let q = q as i32;
    if pos { q } else { -q }
}

fn is_char(it: &Item) -> bool {
    matches!(it, Item::Char { .. })
}

/// `type(p) < math_node`.
fn non_discardable(it: &Item) -> bool {
    matches!(
        it,
        Item::Box(_)
            | Item::Rule { .. }
            | Item::Ins(_)
            | Item::Mark { .. }
            | Item::Adjust(_)
            | Item::Lig { .. }
            | Item::Disc(_)
            | Item::Whatsit(_)
    )
}

fn empty_hbox(it: &Item) -> bool {
    matches!(it, Item::Box(b) if !b.vertical && b.width == 0 && b.list.is_empty())
}

/// pdfTeX `cp_skippable`: nodes invisible to protrusion.
fn cp_skippable(it: &Item) -> bool {
    match it {
        Item::Ins(_) | Item::Mark { .. } | Item::Adjust(_) | Item::Penalty(_) | Item::Whatsit(_) => true,
        Item::Disc(d) => d.pre.is_empty() && d.post.is_empty() && d.replace == 0,
        Item::Kern { w, .. } | Item::MarginKern { w, .. } | Item::Math { w, .. } => *w == 0,
        Item::Glue { g, .. } => g.width == 0 && g.stretch == 0 && g.shrink == 0,
        _ => false,
    }
}

/// pdfTeX `find_protchar_left` (without descending into boxes).
fn prot_left(list: &[Item], mut i: usize, discardable: bool) -> Option<usize> {
    let n = list.len();
    while i + 1 < n && empty_hbox(&list[i]) {
        i += 1;
    }
    if discardable {
        while i + 1 < n && !(is_char(&list[i]) || non_discardable(&list[i])) {
            i += 1;
        }
    }
    while i < n && cp_skippable(&list[i]) {
        i += 1;
    }
    (i < n).then_some(i)
}

/// pdfTeX `find_protchar_right`: the last visible node of `list[lo..end]`.
fn prot_right(list: &[Item], lo: usize, end: usize) -> Option<usize> {
    let mut i = end.min(list.len());
    while i > lo {
        i -= 1;
        if empty_hbox(&list[i]) || cp_skippable(&list[i]) {
            continue;
        }
        return Some(i);
    }
    None
}

/// e-TeX penalty arrays: entry `min(r, n)` (1-based).
fn shape_pen(arr: &[i32], r: i32) -> i32 {
    let n = arr.len() as i32;
    let r = r.min(n).max(1);
    arr[(r - 1) as usize]
}

#[derive(Clone, Copy)]
struct Active {
    fitness: u8,
    hyph: bool,
    line: i32,
    /// Passive node index (`break_node`).
    brk: Option<usize>,
    total: i32,
    short: i32,
    glue: i32,
}

#[derive(Clone, Copy)]
enum A {
    Act(Active),
    Delta([i32; 7]),
}

struct Passive {
    /// Index in the list, `None` for the paragraph's end.
    cur_break: Option<usize>,
    prev_break: Option<usize>,
}

enum Lk {
    Kern(i32),
    Lig(u8, u16),
}

struct St<'a> {
    p: &'a LineParams,
    fonts: &'a dyn Fonts,
    hl: Vec<Item>,
    active: Vec<A>,
    passive: Vec<Passive>,
    active_width: [i32; 7],
    background: [i32; 7],
    break_width: [i32; 7],
    minimal_demerits: [i32; 4],
    minimum_demerits: i32,
    best_place: [Option<usize>; 4],
    best_pl_line: [i32; 4],
    best_pl_short: [i32; 4],
    best_pl_glue: [i32; 4],
    disc_width: i32,
    easy_line: i32,
    last_special_line: i32,
    first_width: i32,
    second_width: i32,
    first_indent: i32,
    second_indent: i32,
    threshold: i32,
    second_pass: bool,
    final_pass: bool,
    do_last_line_fit: bool,
    fill_width: [i32; 3],
    no_shrink_error_yet: bool,
    cur_lang: i32,
    l_hyf: i32,
    r_hyf: i32,
    // hyphenation / reconstitution (tex.web 892, 900, 905, 907)
    hu: [u16; 66],
    hyf: [u8; 66],
    hf: FontId,
    font: Option<Arc<Font>>,
    init_list: Vec<u16>,
    init_lig: bool,
    init_lft: bool,
    hyphen_passed: usize,
    lig_present: bool,
    lft_hit: bool,
    rt_hit: bool,
    hold: Vec<Item>,
    diags: Vec<Arc<Diag>>,
}

pub fn line_break(hlist: Vec<Item>, p: &LineParams, fonts: &dyn crate::font::Fonts) -> Lines {
    let mut hl = hlist;
    // §816: the final glue becomes \penalty10000, then \parfillskip.
    match hl.last() {
        Some(Item::Glue { .. }) => {
            let n = hl.len();
            hl[n - 1] = Item::Penalty(INF_PENALTY);
        }
        _ => hl.push(Item::Penalty(INF_PENALTY)),
    }
    hl.push(Item::Glue { g: p.par_fill_skip, sub: SUB_PARFILLSKIP, leader: None });
    let mut st = St {
        p,
        fonts,
        hl,
        active: Vec::new(),
        passive: Vec::new(),
        active_width: [0; 7],
        background: [0; 7],
        break_width: [0; 7],
        minimal_demerits: [AWFUL_BAD; 4],
        minimum_demerits: AWFUL_BAD,
        best_place: [None; 4],
        best_pl_line: [0; 4],
        best_pl_short: [0; 4],
        best_pl_glue: [0; 4],
        disc_width: 0,
        easy_line: 0,
        last_special_line: 0,
        first_width: 0,
        second_width: 0,
        first_indent: 0,
        second_indent: 0,
        threshold: 0,
        second_pass: false,
        final_pass: false,
        do_last_line_fit: false,
        fill_width: [0; 3],
        no_shrink_error_yet: true,
        cur_lang: p.lang,
        l_hyf: p.lhm,
        r_hyf: p.rhm,
        hu: [0; 66],
        hyf: [0; 66],
        hf: 0,
        font: None,
        init_list: Vec::new(),
        init_lig: false,
        init_lft: false,
        hyphen_passed: 0,
        lig_present: false,
        lft_hit: false,
        rt_hit: false,
        hold: Vec::new(),
        diags: Vec::new(),
    };
    st.setup();
    let best = st.passes();
    st.finish_last_line(best);
    st.post_line_break(best)
}

impl St<'_> {
    fn char_width(&self, f: FontId, c: u16) -> i32 {
        self.fonts.font(f).width(c)
    }

    /// §841, §870: the width of a node in a discretionary.
    fn width(&self, it: &Item) -> i32 {
        match it {
            Item::Char { font, c } | Item::Lig { font, c, .. } => self.char_width(*font, *c),
            Item::Box(b) => b.width,
            Item::Rule { w, .. } | Item::Kern { w, .. } | Item::MarginKern { w, .. } => *w,
            _ => 0,
        }
    }

    /// §826 `check_shrinkage`: true when `g` was made finite.
    fn check_shrinkage(&mut self, g: &mut Glue) -> bool {
        if g.shrink_order != 0 && g.shrink != 0 {
            if self.no_shrink_error_yet {
                self.no_shrink_error_yet = false;
                self.diags.push(Arc::new(Diag {
                    msg: "Infinite glue shrinkage found in a paragraph".into(),
                    help: vec![
                        "The paragraph just ended includes some glue that has".into(),
                        "infinite shrinkability, e.g., `\\hskip 0pt minus 1fil'.".into(),
                        "Such glue doesn't belong there---it allows a paragraph".into(),
                        "of any length to fit on one line. But it's safe to proceed,".into(),
                        "since the offensive shrinkability has been made finite.".into(),
                    ],
                    line: 0,
                }));
            }
            g.shrink_order = 0;
            return true;
        }
        false
    }

    /// §827, e-TeX last line fit, §848.
    fn setup(&mut self) {
        let p = self.p;
        let mut q = p.left_skip;
        let mut r = p.right_skip;
        self.check_shrinkage(&mut q);
        self.check_shrinkage(&mut r);
        let mut bg = [0i32; 7];
        bg[1] = q.width.wrapping_add(r.width);
        bg[2 + q.stretch_order.min(3) as usize] += q.stretch;
        bg[2 + r.stretch_order.min(3) as usize] += r.stretch;
        bg[6] = q.shrink.wrapping_add(r.shrink);
        self.background = bg;

        self.do_last_line_fit = false;
        if p.last_line_fit > 0 {
            let q = p.par_fill_skip;
            if q.stretch > 0 && q.stretch_order > 0 && bg[3] == 0 && bg[4] == 0 && bg[5] == 0 {
                self.do_last_line_fit = true;
                self.fill_width = [0; 3];
                self.fill_width[(q.stretch_order.min(3) - 1) as usize] = q.stretch;
            }
        }

        if p.parshape.is_empty() {
            if p.hang_indent == 0 {
                self.last_special_line = 0;
                self.second_width = p.hsize;
                self.second_indent = 0;
            } else {
                // §849
                self.last_special_line = p.hang_after.abs();
                let hi = p.hang_indent.abs();
                let ind = if p.hang_indent >= 0 { p.hang_indent } else { 0 };
                if p.hang_after < 0 {
                    self.first_width = p.hsize - hi;
                    self.first_indent = ind;
                    self.second_width = p.hsize;
                    self.second_indent = 0;
                } else {
                    self.first_width = p.hsize;
                    self.first_indent = 0;
                    self.second_width = p.hsize - hi;
                    self.second_indent = ind;
                }
            }
        } else {
            let n = p.parshape.len();
            self.last_special_line = n as i32 - 1;
            self.second_indent = p.parshape[n - 1].0;
            self.second_width = p.parshape[n - 1].1;
        }
        self.easy_line = if p.looseness == 0 { self.last_special_line } else { MAX_HALFWORD };
    }

    /// (width, indent) of line `l` (§850, §889).
    fn line_width_of(&self, l: i32) -> (i32, i32) {
        if l > self.last_special_line {
            (self.second_width, self.second_indent)
        } else if self.p.parshape.is_empty() {
            (self.first_width, self.first_indent)
        } else {
            let (ind, w) = self.p.parshape[(l - 1).max(0) as usize];
            (w, ind)
        }
    }

    /// §863: the passes; the index of the best active node.
    fn passes(&mut self) -> usize {
        let p = self.p;
        self.threshold = p.pretolerance;
        if self.threshold >= 0 {
            self.second_pass = false;
            self.final_pass = false;
        } else {
            self.threshold = p.tolerance;
            self.second_pass = true;
            self.final_pass = p.emergency_stretch <= 0;
        }
        loop {
            if self.threshold > INF_BAD {
                self.threshold = INF_BAD;
            }
            if self.second_pass {
                // §891
                self.cur_lang = p.lang;
                self.l_hyf = p.lhm;
                self.r_hyf = p.rhm;
            }
            // §864
            self.active = vec![A::Act(Active {
                fitness: DECENT,
                hyph: false,
                line: p.prev_graf + 1,
                brk: None,
                total: 0,
                short: 0,
                glue: 0,
            })];
            self.active_width = self.background;
            self.passive.clear();

            let mut cur_p: Option<usize> = if self.hl.is_empty() { None } else { Some(0) };
            let mut auto_breaking = true;
            let mut prev_p = 0usize;
            while let Some(mut cp) = cur_p {
                if self.active.is_empty() {
                    break;
                }
                // §867
                if is_char(&self.hl[cp]) {
                    prev_p = cp;
                    while cp < self.hl.len() {
                        let Item::Char { font, c } = self.hl[cp] else { break };
                        self.active_width[1] += self.char_width(font, c);
                        cp += 1;
                    }
                    if cp >= self.hl.len() {
                        cur_p = None;
                        continue;
                    }
                }
                let item = self.hl[cp].clone();
                let mut next = cp + 1;
                match item {
                    Item::Glue { g, sub, leader } => {
                        // §868
                        if auto_breaking {
                            let pp = &self.hl[prev_p];
                            let legal = is_char(pp)
                                || non_discardable(pp)
                                || matches!(pp, Item::Kern { sub, .. } if *sub != 1);
                            if legal {
                                self.try_break(0, false, Some(cp));
                            }
                        }
                        let mut g2 = g;
                        if self.check_shrinkage(&mut g2) {
                            self.hl[cp] = Item::Glue { g: g2, sub, leader };
                        }
                        self.active_width[1] += g2.width;
                        self.active_width[2 + g2.stretch_order.min(3) as usize] += g2.stretch;
                        self.active_width[6] += g2.shrink;
                        if self.second_pass && auto_breaking {
                            self.try_hyphenate(cp);
                        }
                    }
                    Item::Kern { w, sub } => {
                        if sub == 1 {
                            self.kern_break(cp, w, auto_breaking);
                        } else {
                            self.active_width[1] += w;
                        }
                    }
                    Item::MarginKern { w, .. } => self.active_width[1] += w,
                    Item::Lig { font, c, .. } => self.active_width[1] += self.char_width(font, c),
                    Item::Disc(d) => {
                        // §869
                        self.disc_width = 0;
                        if d.pre.is_empty() {
                            self.try_break(p.ex_hyphen_penalty, true, Some(cp));
                        } else {
                            let dw: i32 = d.pre.iter().map(|it| self.width(it)).sum();
                            self.disc_width = dw;
                            self.active_width[1] += dw;
                            self.try_break(p.hyphen_penalty, true, Some(cp));
                            self.active_width[1] -= self.disc_width;
                        }
                        let mut s = cp + 1;
                        for _ in 0..d.replace {
                            if s >= self.hl.len() {
                                break;
                            }
                            self.active_width[1] += self.width(&self.hl[s]);
                            s += 1;
                        }
                        next = s;
                    }
                    Item::Math { w, sub } => {
                        if sub < L_CODE {
                            auto_breaking = sub & 1 == 1;
                        }
                        self.kern_break(cp, w, auto_breaking);
                    }
                    Item::Penalty(n) => self.try_break(n, false, Some(cp)),
                    Item::Whatsit(w) => {
                        if let Whatsit::Language { lang, lhm, rhm } = &*w {
                            self.cur_lang = *lang as i32;
                            self.l_hyf = *lhm as i32;
                            self.r_hyf = *rhm as i32;
                        }
                    }
                    Item::Box(b) => self.active_width[1] += b.width,
                    Item::Rule { w, .. } => self.active_width[1] += w,
                    _ => {}
                }
                prev_p = cp;
                cur_p = (next < self.hl.len()).then_some(next);
            }
            if cur_p.is_none() {
                // §873
                self.try_break(EJECT_PENALTY, true, None);
                if !self.active.is_empty() {
                    // §874
                    let mut fewest = AWFUL_BAD;
                    let mut best = 0usize;
                    for (i, a) in self.active.iter().enumerate() {
                        if let A::Act(x) = a {
                            if x.total < fewest {
                                fewest = x.total;
                                best = i;
                            }
                        }
                    }
                    if p.looseness == 0 {
                        return best;
                    }
                    // §875
                    let best_line = self.act(best).line;
                    let mut actual = 0;
                    for (i, a) in self.active.iter().enumerate() {
                        if let A::Act(x) = a {
                            let ld = x.line - best_line;
                            if (ld < actual && p.looseness <= ld) || (ld > actual && p.looseness >= ld) {
                                best = i;
                                actual = ld;
                                fewest = x.total;
                            } else if ld == actual && x.total < fewest {
                                best = i;
                                fewest = x.total;
                            }
                        }
                    }
                    if actual == p.looseness || self.final_pass {
                        return best;
                    }
                }
            }
            // §873: clean up and try again
            if !self.second_pass {
                self.threshold = p.tolerance;
                self.second_pass = true;
                self.final_pass = p.emergency_stretch <= 0;
            } else {
                self.background[2] += p.emergency_stretch;
                self.final_pass = true;
            }
        }
    }

    fn act(&self, i: usize) -> Active {
        match self.active[i] {
            A::Act(a) => a,
            A::Delta(_) => unreachable!("delta node where an active node is expected"),
        }
    }

    /// `kern_break` (§866).
    fn kern_break(&mut self, cp: usize, w: i32, auto_breaking: bool) {
        if auto_breaking && matches!(self.hl.get(cp + 1), Some(Item::Glue { .. })) {
            self.try_break(0, false, Some(cp));
        }
        self.active_width[1] += w;
    }

    /// e-TeX: the last line's glue as `\lastlinefit` set it.
    fn finish_last_line(&mut self, best: usize) {
        if !self.do_last_line_fit {
            return;
        }
        let a = self.act(best);
        if a.short == 0 {
            self.do_last_line_fit = false;
            return;
        }
        if let Some(Item::Glue { g, .. }) = self.hl.last_mut() {
            g.width += a.short - a.glue;
            g.stretch = 0;
        }
    }

    /// §837, §840: `break_width` for a break at `cur_p`.
    fn compute_break_width(&mut self, hyph: bool, cur_p: Option<usize>) {
        self.break_width = self.background;
        let Some(cp) = cur_p else { return };
        let mut s = Some(cp);
        if hyph {
            if let Item::Disc(d) = &self.hl[cp] {
                let d = d.clone();
                let mut v = cp;
                for _ in 0..d.replace {
                    v += 1;
                    if v >= self.hl.len() {
                        break;
                    }
                    let w = self.width(&self.hl[v]);
                    self.break_width[1] -= w;
                }
                let pw: i32 = d.post.iter().map(|it| self.width(it)).sum();
                self.break_width[1] += pw + self.disc_width;
                s = if d.post.is_empty() { Some(v + 1) } else { None };
            }
        }
        while let Some(i) = s {
            if i >= self.hl.len() {
                break;
            }
            match &self.hl[i] {
                Item::Glue { g, .. } => {
                    self.break_width[1] -= g.width;
                    self.break_width[2 + g.stretch_order.min(3) as usize] -= g.stretch;
                    self.break_width[6] -= g.shrink;
                }
                Item::Penalty(_) => {}
                Item::Math { w, .. } => self.break_width[1] -= w,
                Item::Kern { w, sub } => {
                    if *sub != 1 {
                        break;
                    }
                    self.break_width[1] -= w;
                }
                _ => break,
            }
            s = Some(i + 1);
        }
    }

    /// pdfTeX `total_pw`: protrusion at both ends of a line from `r` to `cur_p`.
    fn total_pw(&self, r_brk: Option<usize>, cur_p: Option<usize>) -> i32 {
        let hl = &self.hl;
        if hl.is_empty() {
            return 0;
        }
        let start = match r_brk {
            None => Some(0),
            Some(pi) => self.passive[pi].cur_break,
        };
        let Some(start) = start else { return 0 };
        let right = match cur_p {
            Some(pp) => match &hl[pp] {
                Item::Disc(d) if !d.pre.is_empty() => self.char_pw(d.pre.last(), false),
                _ => self.char_pw(prot_right(hl, start, pp).map(|i| &hl[i]), false),
            },
            None => self.char_pw(prot_right(hl, start, hl.len()).map(|i| &hl[i]), false),
        };
        let left = match &hl[start] {
            Item::Disc(d) if r_brk.is_some() => {
                if !d.post.is_empty() {
                    self.char_pw(d.post.first(), true)
                } else {
                    let i = (start + 1 + d.replace as usize).min(hl.len() - 1);
                    self.char_pw(prot_left(hl, i, true).map(|i| &hl[i]), true)
                }
            }
            _ => self.char_pw(prot_left(hl, start, true).map(|i| &hl[i]), true),
        };
        left + right
    }

    /// pdfTeX `char_pw`.
    fn char_pw(&self, it: Option<&Item>, left: bool) -> i32 {
        let (f, c) = match it {
            Some(Item::Char { font, c }) | Some(Item::Lig { font, c, .. }) => (*font, *c),
            _ => return 0,
        };
        let map = if left { &self.p.lp_codes } else { &self.p.rp_codes };
        let code = map.get(&(f, c)).copied().unwrap_or(0);
        if code == 0 {
            return 0;
        }
        round_xn_over_d(self.fonts.font(f).param(6), code, 1000)
    }

    /// e-TeX: badness of the last line under `\lastlinefit`, or `None` (not_found).
    fn last_line(&self, short: i32, glue: i32, caw: &[i32; 7], shortfall: i32) -> Option<(i32, u8, i32)> {
        if short == 0 || glue <= 0 {
            return None;
        }
        if caw[3] != self.fill_width[0] || caw[4] != self.fill_width[1] || caw[5] != self.fill_width[2] {
            return None;
        }
        let mut g = if short > 0 { caw[2] } else { caw[6] };
        if g <= 0 {
            return None;
        }
        let mut err = false;
        match fract(g, short, glue, MAX_DIMEN) {
            Some(v) => g = v,
            None => {
                err = true;
                g = 0;
            }
        }
        if self.p.last_line_fit < 1000 {
            match fract(g, self.p.last_line_fit, 1000, MAX_DIMEN) {
                Some(v) => g = v,
                None => {
                    err = true;
                    g = 0;
                }
            }
        }
        if err {
            g = if short > 0 { MAX_DIMEN } else { -MAX_DIMEN };
        }
        if g > 0 {
            if g > shortfall {
                g = shortfall;
            }
            if g > 7_230_584 && caw[2] < 1_663_497 {
                return Some((INF_BAD, VERY_LOOSE, g));
            }
            let b = badness(g, caw[2]);
            let fit = if b > 12 {
                if b > 99 { VERY_LOOSE } else { LOOSE }
            } else {
                DECENT
            };
            Some((b, fit, g))
        } else if g < 0 {
            if -g > caw[6] {
                g = -caw[6];
            }
            let b = badness(-g, caw[6]);
            Some((b, if b > 12 { TIGHT } else { DECENT }, g))
        } else {
            None
        }
    }

    /// §829 `try_break`.
    fn try_break(&mut self, pi: i32, hyph: bool, cur_p: Option<usize>) {
        let p = self.p;
        let mut pi = pi;
        if pi.abs() >= INF_PENALTY {
            if pi > 0 {
                return;
            }
            pi = EJECT_PENALTY;
        }
        let mut no_break_yet = true;
        let mut prev_r: Option<usize> = None;
        let mut old_l = 0i32;
        let mut caw = self.active_width;
        let mut line_width = 0i32;
        loop {
            let mut r = prev_r.map_or(0, |x| x + 1);
            // §832
            if let Some(A::Delta(d)) = self.active.get(r) {
                for k in 1..7 {
                    caw[k] = caw[k].wrapping_add(d[k]);
                }
                prev_r = Some(r);
                continue;
            }
            // §835
            let l = if r < self.active.len() { self.act(r).line } else { MAX_HALFWORD };
            if l > old_l {
                if self.minimum_demerits < AWFUL_BAD && (old_l != self.easy_line || r == self.active.len()) {
                    // §836
                    if no_break_yet {
                        no_break_yet = false;
                        self.compute_break_width(hyph, cur_p);
                    }
                    // §843
                    match prev_r {
                        Some(pr) if matches!(self.active[pr], A::Delta(_)) => {
                            let bw = self.break_width;
                            if let A::Delta(d) = &mut self.active[pr] {
                                for k in 1..7 {
                                    d[k] = d[k] - caw[k] + bw[k];
                                }
                            }
                        }
                        None => self.active_width = self.break_width,
                        Some(pr) => {
                            let mut d = [0i32; 7];
                            for k in 1..7 {
                                d[k] = self.break_width[k] - caw[k];
                            }
                            self.active.insert(pr + 1, A::Delta(d));
                            prev_r = Some(pr + 1);
                            r += 1;
                        }
                    }
                    if p.adj_demerits.abs() >= AWFUL_BAD - self.minimum_demerits {
                        self.minimum_demerits = AWFUL_BAD - 1;
                    } else {
                        self.minimum_demerits += p.adj_demerits.abs();
                    }
                    for fc in VERY_LOOSE..=TIGHT {
                        let f = fc as usize;
                        if self.minimal_demerits[f] <= self.minimum_demerits {
                            // §845
                            self.passive.push(Passive { cur_break: cur_p, prev_break: self.best_place[f] });
                            let q = Active {
                                fitness: fc,
                                hyph,
                                line: self.best_pl_line[f] + 1,
                                brk: Some(self.passive.len() - 1),
                                total: self.minimal_demerits[f],
                                short: if self.do_last_line_fit { self.best_pl_short[f] } else { 0 },
                                glue: if self.do_last_line_fit { self.best_pl_glue[f] } else { 0 },
                            };
                            let pos = prev_r.map_or(0, |x| x + 1);
                            self.active.insert(pos, A::Act(q));
                            prev_r = Some(pos);
                            r += 1;
                        }
                        self.minimal_demerits[f] = AWFUL_BAD;
                    }
                    self.minimum_demerits = AWFUL_BAD;
                    // §844
                    if r != self.active.len() {
                        let mut d = [0i32; 7];
                        for k in 1..7 {
                            d[k] = caw[k] - self.break_width[k];
                        }
                        let pos = prev_r.map_or(0, |x| x + 1);
                        self.active.insert(pos, A::Delta(d));
                        prev_r = Some(pos);
                        r += 1;
                    }
                }
                if r == self.active.len() {
                    return;
                }
                // §850
                if l > self.easy_line {
                    line_width = self.second_width;
                    old_l = MAX_HALFWORD - 1;
                } else {
                    old_l = l;
                    line_width = self.line_width_of(l).0;
                }
            }
            // §851
            let ra = self.act(r);
            let mut artificial = false;
            let mut shortfall = line_width.wrapping_sub(caw[1]);
            if p.protrude_chars > 1 {
                shortfall = shortfall.wrapping_add(self.total_pw(ra.brk, cur_p));
            }
            let mut b = 0i32;
            let mut fit = DECENT;
            let mut g = 0i32;
            let mut found = false;
            if shortfall > 0 {
                // §852
                if caw[3] != 0 || caw[4] != 0 || caw[5] != 0 {
                    if self.do_last_line_fit {
                        if cur_p.is_none() {
                            if let Some((bb, ff, gg)) = self.last_line(ra.short, ra.glue, &caw, shortfall) {
                                b = bb;
                                fit = ff;
                                g = gg;
                                found = true;
                            }
                        }
                        if !found {
                            shortfall = 0;
                        }
                    }
                    if !found {
                        b = 0;
                        fit = DECENT;
                    }
                } else if shortfall > 7_230_584 && caw[2] < 1_663_497 {
                    b = INF_BAD;
                    fit = VERY_LOOSE;
                } else {
                    b = badness(shortfall, caw[2]);
                    fit = if b > 12 {
                        if b > 99 { VERY_LOOSE } else { LOOSE }
                    } else {
                        DECENT
                    };
                }
            } else {
                // §853
                b = if -shortfall > caw[6] { INF_BAD + 1 } else { badness(-shortfall, caw[6]) };
                fit = if b > 12 { TIGHT } else { DECENT };
            }
            if self.do_last_line_fit && !found {
                if cur_p.is_none() {
                    shortfall = 0;
                }
                g = if shortfall > 0 {
                    caw[2]
                } else if shortfall < 0 {
                    caw[6]
                } else {
                    0
                };
            }
            // found:
            let node_r_stays_active;
            let mut deactivate = false;
            if b > INF_BAD || pi == EJECT_PENALTY {
                // §854
                if self.final_pass
                    && self.minimum_demerits == AWFUL_BAD
                    && r + 1 == self.active.len()
                    && prev_r.is_none()
                {
                    artificial = true;
                } else if b > self.threshold {
                    deactivate = true;
                }
                node_r_stays_active = false;
            } else {
                prev_r = Some(r);
                if b > self.threshold {
                    continue;
                }
                node_r_stays_active = true;
            }
            if !deactivate {
                // §855
                let mut d = if artificial {
                    0
                } else {
                    // §859
                    let mut d = p.line_penalty.wrapping_add(b);
                    d = if d.abs() >= 10000 { 100_000_000 } else { d * d };
                    if pi != 0 {
                        if pi > 0 {
                            d = d.wrapping_add(pi * pi);
                        } else if pi > EJECT_PENALTY {
                            d = d.wrapping_sub(pi * pi);
                        }
                    }
                    if hyph && ra.hyph {
                        d = d.wrapping_add(if cur_p.is_some() {
                            p.double_hyphen_demerits
                        } else {
                            p.final_hyphen_demerits
                        });
                    }
                    if (fit as i32 - ra.fitness as i32).abs() > 1 {
                        d = d.wrapping_add(p.adj_demerits);
                    }
                    d
                };
                d = d.wrapping_add(ra.total);
                let f = fit as usize;
                if d <= self.minimal_demerits[f] {
                    self.minimal_demerits[f] = d;
                    self.best_place[f] = ra.brk;
                    self.best_pl_line[f] = l;
                    if self.do_last_line_fit {
                        self.best_pl_short[f] = shortfall;
                        self.best_pl_glue[f] = g;
                    }
                    if d < self.minimum_demerits {
                        self.minimum_demerits = d;
                    }
                }
                if node_r_stays_active {
                    continue;
                }
            }
            // §860: deactivate node r
            self.active.remove(r);
            match prev_r {
                None => {
                    // §861
                    if let Some(A::Delta(d)) = self.active.first().copied() {
                        for k in 1..7 {
                            self.active_width[k] = self.active_width[k].wrapping_add(d[k]);
                        }
                        caw = self.active_width;
                        self.active.remove(0);
                    }
                }
                Some(pr) => {
                    if let A::Delta(dp) = self.active[pr] {
                        let r2 = pr + 1;
                        if r2 == self.active.len() {
                            for k in 1..7 {
                                caw[k] = caw[k].wrapping_sub(dp[k]);
                            }
                            self.active.remove(pr);
                            prev_r = pr.checked_sub(1);
                        } else if let A::Delta(dr) = self.active[r2] {
                            let mut nd = dp;
                            for k in 1..7 {
                                caw[k] = caw[k].wrapping_add(dr[k]);
                                nd[k] = nd[k].wrapping_add(dr[k]);
                            }
                            self.active[pr] = A::Delta(nd);
                            self.active.remove(r2);
                        }
                    }
                }
            }
        }
    }

    // ---- hyphenation (§894–903) and reconstitution (§905–918) ----

    fn lc_code(&self, c: u16) -> u16 {
        self.p.lc.get(&c).copied().unwrap_or(0)
    }

    fn font_bchar(&self) -> u16 {
        self.font.as_ref().and_then(|f| f.bchar()).unwrap_or(NON)
    }

    /// The lig/kern instruction for (`l`, `t`); `NON` as `l` is the left boundary.
    fn lookup(&self, l: u16, t: u16) -> Option<Lk> {
        if t == NON {
            return None;
        }
        let f = self.font.as_ref()?;
        let r = if l == NON { f.lig_kern_boundary(None, Some(t)) } else { f.lig_kern(l, t) };
        match r {
            LigKern::None => None,
            LigKern::Kern(w) => Some(Lk::Kern(w)),
            LigKern::Lig { op, c } => Some(Lk::Lig(op, c)),
        }
    }

    /// §894: try to hyphenate the word after the glue at `cp`.
    fn try_hyphenate(&mut self, cp: usize) {
        let n = self.hl.len();
        let mut prev_s = cp;
        let mut s = cp + 1;
        if s >= n {
            return;
        }
        // §896
        let hf: FontId;
        loop {
            if s >= n {
                return;
            }
            let (c, f) = match &self.hl[s] {
                Item::Char { font, c } => (*c, *font),
                Item::Lig { font, orig, .. } => {
                    if orig.is_empty() {
                        prev_s = s;
                        s += 1;
                        continue;
                    }
                    (orig[0], *font)
                }
                Item::Kern { sub: 0, .. } => {
                    prev_s = s;
                    s += 1;
                    continue;
                }
                Item::Math { sub, .. } if *sub >= L_CODE => {
                    prev_s = s;
                    s += 1;
                    continue;
                }
                Item::Whatsit(w) => {
                    if let Whatsit::Language { lang, lhm, rhm } = &**w {
                        self.cur_lang = *lang as i32;
                        self.l_hyf = *lhm as i32;
                        self.r_hyf = *rhm as i32;
                    }
                    prev_s = s;
                    s += 1;
                    continue;
                }
                _ => return,
            };
            let lc = self.lc_code(c);
            if lc != 0 {
                if lc == c || self.p.uc_hyph > 0 {
                    hf = f;
                    break;
                }
                return;
            }
            prev_s = s;
            s += 1;
        }
        let hyf_char = self.p.hyphen_chars.get(&hf).copied().unwrap_or(-1);
        if !(0..=255).contains(&hyf_char) {
            return;
        }
        let hyf_char = hyf_char as u16;
        let ha = prev_s;
        if self.l_hyf + self.r_hyf > 63 {
            return;
        }
        self.hf = hf;
        self.font = Some(self.fonts.font(hf));
        let font_bchar = self.font_bchar();
        // §897
        let mut hn = 0usize;
        let mut hc = [0u16; 66];
        let mut hb = s;
        let mut hyf_bchar = NON;
        'done3: loop {
            if s >= n {
                break;
            }
            match &self.hl[s] {
                Item::Char { font, c } => {
                    if *font != hf {
                        break;
                    }
                    hyf_bchar = *c;
                    let lc = self.lc_code(*c);
                    if lc == 0 || hn == 63 {
                        break;
                    }
                    hb = s;
                    hn += 1;
                    self.hu[hn] = *c;
                    hc[hn] = lc;
                    hyf_bchar = NON;
                }
                Item::Lig { font, orig, right, .. } => {
                    // §898
                    if *font != hf {
                        break;
                    }
                    let mut j = hn;
                    if let Some(&c0) = orig.first() {
                        hyf_bchar = c0;
                    }
                    for &c in orig.iter() {
                        let lc = self.lc_code(c);
                        if lc == 0 || j == 63 {
                            break 'done3;
                        }
                        j += 1;
                        self.hu[j] = c;
                        hc[j] = lc;
                    }
                    hb = s;
                    hn = j;
                    hyf_bchar = if *right { font_bchar } else { NON };
                }
                Item::Kern { sub: 0, .. } => {
                    hb = s;
                    hyf_bchar = font_bchar;
                }
                _ => break,
            }
            s += 1;
        }
        // §899
        if (hn as i32) < self.l_hyf + self.r_hyf {
            return;
        }
        loop {
            if s >= n {
                return;
            }
            match &self.hl[s] {
                Item::Char { .. } | Item::Lig { .. } => {}
                Item::Kern { sub, .. } => {
                    if *sub != 0 {
                        break;
                    }
                }
                Item::Whatsit(_)
                | Item::Glue { .. }
                | Item::Penalty(_)
                | Item::Ins(_)
                | Item::Adjust(_)
                | Item::Mark { .. } => break,
                Item::Math { sub, .. } => {
                    if *sub >= L_CODE {
                        break;
                    }
                    return;
                }
                _ => return,
            }
            s += 1;
        }
        // §923: hyphen positions (hyf[j] odd: a hyphen after hu[j])
        if !(0..=255).contains(&self.cur_lang) {
            return;
        }
        let Some((pat, exc)) = self.p.langs.get(&(self.cur_lang as u8)) else { return };
        let (lh, rh) = (self.l_hyf.max(1) as usize, self.r_hyf.max(1) as usize);
        let pos = crate::hyph::hyphenate(&hc[1..=hn], pat.as_ref(), exc.as_ref(), lh, rh);
        self.hyf = [0; 66];
        let mut any = false;
        for j in pos {
            if j >= lh && j + rh <= hn {
                self.hyf[j] = 1;
                any = true;
            }
        }
        if !any {
            return;
        }
        // §903
        let start;
        let j0;
        let found2 = |st: &mut Self| {
            st.hu[0] = NON;
            st.init_lig = false;
            st.init_list.clear();
        };
        match self.hl[ha].clone() {
            Item::Char { font, c } if font == hf => {
                self.init_list = vec![c];
                self.init_lig = false;
                self.hu[0] = c;
                start = ha;
                j0 = 0;
            }
            Item::Lig { font, c, orig, left, .. } if font == hf => {
                self.init_list = orig.to_vec();
                self.init_lig = true;
                self.init_lft = left;
                self.hu[0] = c;
                if self.init_list.is_empty() && left {
                    self.hu[0] = NON;
                    self.init_lig = false;
                }
                start = ha;
                j0 = 0;
            }
            Item::Char { .. } | Item::Lig { .. } => {
                found2(self);
                start = ha + 1;
                j0 = 0;
            }
            _ => {
                if matches!(self.hl.get(ha + 1), Some(Item::Lig { left: true, .. })) {
                    found2(self);
                    j0 = 0;
                } else {
                    self.init_list.clear();
                    j0 = 1;
                }
                start = ha + 1;
            }
        }
        let out = self.reconstitute_word(j0, hn, hyf_bchar, hyf_char, font_bchar);
        self.init_list.clear();
        self.hl.splice(start..=hb, out);
    }

    /// §913–918: the word's new nodes.
    fn reconstitute_word(&mut self, j0: usize, hn: usize, bchar: u16, hyf_char: u16, font_bchar: u16) -> Vec<Item> {
        let font = self.font.clone().expect("hyphenation font");
        let mut out: Vec<Item> = Vec::new();
        let mut j = j0;
        loop {
            let mut l = j;
            j = self.reconstitute(j, hn, bchar, hyf_char) + 1;
            if self.hyphen_passed == 0 {
                out.append(&mut self.hold);
                if self.hyf[j - 1] & 1 == 1 {
                    l = j;
                    self.hyphen_passed = j - 1;
                    self.hold.clear();
                }
            }
            if self.hyphen_passed > 0 {
                // §914
                loop {
                    let mut major: Vec<Item> = std::mem::take(&mut self.hold);
                    let mut r_count = major.len();
                    let mut i = self.hyphen_passed;
                    self.hyf[i] = 0;
                    // §915
                    let mut pre: Vec<Item> = Vec::new();
                    let hyf_node = font.has(hyf_char);
                    let mut c = 0u16;
                    if hyf_node {
                        i += 1;
                        c = self.hu[i];
                        self.hu[i] = hyf_char;
                    }
                    while l <= i {
                        l = self.reconstitute(l, i, font_bchar, NON) + 1;
                        pre.append(&mut self.hold);
                    }
                    if hyf_node {
                        self.hu[i] = c;
                        l = i;
                    }
                    // §916
                    let mut post: Vec<Item> = Vec::new();
                    let mut c_loc = 0usize;
                    if font.has_left_boundary() {
                        l -= 1;
                        c = self.hu[l];
                        c_loc = l;
                        self.hu[l] = NON;
                    }
                    while l < j {
                        loop {
                            l = self.reconstitute(l, hn, bchar, NON) + 1;
                            if c_loc > 0 {
                                self.hu[c_loc] = c;
                                c_loc = 0;
                            }
                            post.append(&mut self.hold);
                            if l >= j {
                                break;
                            }
                        }
                        while l > j {
                            // §917
                            j = self.reconstitute(j, hn, bchar, NON) + 1;
                            r_count += self.hold.len();
                            major.append(&mut self.hold);
                        }
                    }
                    // §918
                    if r_count <= 127 {
                        out.push(Item::Disc(Arc::new(Disc {
                            pre: pre.into(),
                            post: post.into(),
                            replace: r_count as u16,
                            explicit: false,
                        })));
                    }
                    out.append(&mut major);
                    self.hyphen_passed = j - 1;
                    self.hold.clear();
                    if self.hyf[j - 1] & 1 == 0 {
                        break;
                    }
                }
            }
            if j > hn {
                break;
            }
        }
        out
    }

    /// §905 `reconstitute`: builds `self.hold` from `hu[j..]`; returns the last index used.
    fn reconstitute(&mut self, mut j: usize, n: usize, mut bchar: u16, mut hchar: u16) -> usize {
        let hf = self.hf;
        self.hyphen_passed = 0;
        let mut t: Vec<Item> = Vec::new();
        let mut w = 0i32;
        // §908
        let mut cur_l = self.hu[j];
        let mut cur_q = 0usize;
        if j == 0 {
            self.lig_present = self.init_lig;
            if self.lig_present {
                self.lft_hit = self.init_lft;
            }
            for &c in &self.init_list {
                t.push(Item::Char { font: hf, c });
            }
        } else if cur_l < NON {
            t.push(Item::Char { font: hf, c: cur_l });
        }
        let mut lig_stack: Vec<(u16, Option<u16>)> = Vec::new();
        let set_cur_r = |st: &Self, j: usize, bchar: u16, hchar: u16| -> (u16, u16) {
            let cur_r = if j < n { st.hu[j + 1] } else { bchar };
            let cur_rh = if st.hyf[j] & 1 == 1 { hchar } else { NON };
            (cur_r, cur_rh)
        };
        let (mut cur_r, mut cur_rh) = set_cur_r(self, j, bchar, hchar);
        'cont: loop {
            // §909
            'lk: loop {
                let test = if cur_rh < NON { cur_rh } else { cur_r };
                match self.lookup(cur_l, test) {
                    None => {
                        if cur_rh == NON {
                            break 'lk;
                        }
                        cur_rh = NON;
                        continue 'lk;
                    }
                    Some(_) if cur_rh < NON => {
                        self.hyphen_passed = j;
                        hchar = NON;
                        cur_rh = NON;
                        continue 'lk;
                    }
                    Some(x) => {
                        if hchar < NON && self.hyf[j] & 1 == 1 {
                            self.hyphen_passed = j;
                            hchar = NON;
                        }
                        match x {
                            Lk::Kern(k) => {
                                w = k;
                                break 'lk;
                            }
                            Lk::Lig(op, c) => {
                                // §911
                                if cur_l == NON {
                                    self.lft_hit = true;
                                }
                                if j == n && lig_stack.is_empty() {
                                    self.rt_hit = true;
                                }
                                match op {
                                    1 | 5 => {
                                        cur_l = c;
                                        self.lig_present = true;
                                    }
                                    2 | 6 => {
                                        cur_r = c;
                                        if let Some(top) = lig_stack.last_mut() {
                                            top.0 = c;
                                        } else {
                                            let lp = if j == n {
                                                bchar = NON;
                                                None
                                            } else {
                                                Some(self.hu[j + 1])
                                            };
                                            lig_stack.push((c, lp));
                                        }
                                    }
                                    3 => {
                                        cur_r = c;
                                        lig_stack.push((c, None));
                                    }
                                    7 | 11 => {
                                        self.wrap_lig(&mut t, cur_q, cur_l, false, lig_stack.is_empty());
                                        cur_q = t.len();
                                        cur_l = c;
                                        self.lig_present = true;
                                    }
                                    _ => {
                                        cur_l = c;
                                        self.lig_present = true;
                                        if !lig_stack.is_empty() {
                                            // pop_lig_stack
                                            let (_, lp) = lig_stack.pop().unwrap();
                                            if let Some(ch) = lp {
                                                t.push(Item::Char { font: hf, c: ch });
                                                j += 1;
                                            }
                                            if let Some(top) = lig_stack.last() {
                                                cur_r = top.0;
                                            } else {
                                                (cur_r, cur_rh) = set_cur_r(self, j, bchar, hchar);
                                            }
                                        } else if j == n {
                                            break 'lk;
                                        } else {
                                            t.push(Item::Char { font: hf, c: cur_r });
                                            j += 1;
                                            (cur_r, cur_rh) = set_cur_r(self, j, bchar, hchar);
                                        }
                                    }
                                }
                                if op > 4 && op != 7 {
                                    break 'lk;
                                }
                                continue 'lk;
                            }
                        }
                    }
                }
            }
            // §910
            let rt = self.rt_hit;
            self.wrap_lig(&mut t, cur_q, cur_l, rt, lig_stack.is_empty());
            if w != 0 {
                t.push(Item::Kern { w, sub: 0 });
                w = 0;
            }
            if let Some(&(top, _)) = lig_stack.last() {
                cur_q = t.len();
                cur_l = top;
                self.lig_present = true;
                let (_, lp) = lig_stack.pop().unwrap();
                if let Some(ch) = lp {
                    t.push(Item::Char { font: hf, c: ch });
                    j += 1;
                }
                if let Some(top) = lig_stack.last() {
                    cur_r = top.0;
                } else {
                    (cur_r, cur_rh) = set_cur_r(self, j, bchar, hchar);
                }
                continue 'cont;
            }
            break;
        }
        self.hold = t;
        j
    }

    /// `wrap_lig`: the characters after `cur_q` become a ligature `cur_l`.
    fn wrap_lig(&mut self, t: &mut Vec<Item>, cur_q: usize, cur_l: u16, flag: bool, stack_empty: bool) {
        if !self.lig_present {
            return;
        }
        let cq = cur_q.min(t.len());
        let orig: Vec<u16> = t
            .drain(cq..)
            .filter_map(|it| match it {
                Item::Char { c, .. } => Some(c),
                _ => None,
            })
            .collect();
        let mut left = false;
        let mut right = false;
        if self.lft_hit {
            left = true;
            self.lft_hit = false;
        }
        if flag && stack_empty {
            right = true;
            self.rt_hit = false;
        }
        t.push(Item::Lig { font: self.hf, c: cur_l, orig: orig.into(), left, right });
        self.lig_present = false;
    }

    // ---- post_line_break (§877–890) ----

    fn post_line_break(mut self, best: usize) -> Lines {
        let p = self.p;
        let a = self.act(best);
        let best_line = a.line;
        let mut breaks: Vec<Option<usize>> = Vec::new();
        let mut pb = a.brk;
        while let Some(i) = pb {
            breaks.push(self.passive[i].cur_break);
            pb = self.passive[i].prev_break;
        }
        breaks.reverse();

        let hl = std::mem::take(&mut self.hl);
        let len = hl.len();
        let mut out: Vec<Item> = Vec::new();
        let mut prev_depth = p.prev_depth;
        let mut last: Option<Arc<BoxNode>> = None;
        let mut pos = 0usize;
        let mut front: Vec<Item> = Vec::new();
        let mut cur_line = p.prev_graf + 1;
        let rs = Item::Glue { g: p.right_skip, sub: SUB_RIGHTSKIP, leader: None };
        for (bi, brk) in breaks.iter().enumerate() {
            // §881
            let mut line: Vec<Item> = std::mem::take(&mut front);
            let mut disc_break = false;
            let mut post_disc_break = false;
            match *brk {
                Some(i) => match &hl[i] {
                    Item::Glue { .. } => {
                        line.extend_from_slice(&hl[pos..i]);
                        pos = i + 1;
                    }
                    Item::Disc(d) => {
                        // §882
                        line.extend_from_slice(&hl[pos..i]);
                        line.push(Item::Disc(Arc::new(Disc {
                            pre: Arc::from(Vec::<Item>::new()),
                            post: Arc::from(Vec::<Item>::new()),
                            replace: 0,
                            explicit: d.explicit,
                        })));
                        line.extend_from_slice(&d.pre);
                        if !d.post.is_empty() {
                            front = d.post.to_vec();
                            post_disc_break = true;
                        }
                        disc_break = true;
                        pos = (i + 1 + d.replace as usize).min(len);
                    }
                    Item::Math { sub, .. } => {
                        line.extend_from_slice(&hl[pos..i]);
                        line.push(Item::Math { w: 0, sub: *sub });
                        pos = i + 1;
                    }
                    Item::Kern { sub, .. } => {
                        line.extend_from_slice(&hl[pos..i]);
                        line.push(Item::Kern { w: 0, sub: *sub });
                        pos = i + 1;
                    }
                    _ => {
                        line.extend_from_slice(&hl[pos..=i]);
                        pos = i + 1;
                    }
                },
                None => {
                    line.extend_from_slice(&hl[pos..]);
                    pos = len;
                }
            }
            // pdfTeX: right margin kern, then \rightskip (§886)
            if p.protrude_chars > 0 {
                if let Some(i) = prot_right(&line, 0, line.len()) {
                    let w = self.char_pw(Some(&line[i]), false);
                    if w != 0 {
                        if let Item::Char { font, c } | Item::Lig { font, c, .. } = &line[i] {
                            let (font, c) = (*font, *c);
                            line.push(Item::MarginKern { w: -w, right: true, font, c });
                        }
                    }
                }
            }
            line.push(rs.clone());
            // §887: left margin kern, \leftskip
            if p.protrude_chars > 0 {
                if let Some(i) = prot_left(&line, 0, false) {
                    let w = self.char_pw(Some(&line[i]), true);
                    if w != 0 {
                        if let Item::Char { font, c } | Item::Lig { font, c, .. } = &line[i] {
                            let (font, c) = (*font, *c);
                            line.insert(0, Item::MarginKern { w: -w, right: false, font, c });
                        }
                    }
                }
            }
            if p.left_skip != Glue::default() {
                line.insert(0, Item::Glue { g: p.left_skip, sub: SUB_LEFTSKIP, leader: None });
            }
            // §889
            let (cur_width, cur_indent) = self.line_width_of(cur_line);
            let packed = pack::hpack(line, pack::Spec::Exactly(cur_width), &p.pack, self.fonts);
            if let Some(d) = packed.report.clone() {
                self.diags.push(d);
            }
            let mut b = packed.b;
            b.shift = cur_indent;
            let b = Arc::new(b);
            // §888
            append_to_vlist(&mut out, &mut prev_depth, b.clone(), p.baseline_skip, p.line_skip, p.line_skip_limit);
            last = Some(b);
            out.extend(packed.adjust);
            // §890
            if cur_line + 1 != best_line {
                let mut pen = if p.inter_line_penalties.is_empty() {
                    p.inter_line_penalty
                } else {
                    shape_pen(&p.inter_line_penalties, cur_line)
                };
                if !p.club_penalties.is_empty() {
                    pen = pen.wrapping_add(shape_pen(&p.club_penalties, cur_line - p.prev_graf));
                } else if cur_line == p.prev_graf + 1 {
                    pen = pen.wrapping_add(p.club_penalty);
                }
                let wp = if p.display { &p.display_widow_penalties } else { &p.widow_penalties };
                if !wp.is_empty() {
                    pen = pen.wrapping_add(shape_pen(wp, best_line - cur_line - 1));
                } else if cur_line + 2 == best_line {
                    pen = pen.wrapping_add(if p.display { p.display_widow_penalty } else { p.widow_penalty });
                }
                if disc_break {
                    pen = pen.wrapping_add(p.broken_penalty);
                }
                if pen != 0 {
                    out.push(Item::Penalty(pen));
                }
            }
            cur_line += 1;
            // §879: prune discardable nodes at the start of the next line
            if bi + 1 < breaks.len() && !post_disc_break {
                let next = breaks[bi + 1];
                while pos < len && Some(pos) != next {
                    match &hl[pos] {
                        Item::Glue { .. } | Item::Penalty(_) | Item::Math { .. } => pos += 1,
                        Item::Kern { sub: 1, .. } => pos += 1,
                        _ => break,
                    }
                }
            }
        }
        Lines { items: out, prev_graf: best_line - 1, last, diags: self.diags }
    }
}
