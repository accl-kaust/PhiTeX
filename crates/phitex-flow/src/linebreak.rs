//! Line breaking: a pure function of a paragraph (tex.web parts 38–39,
//! §813–§890, without hyphenation: an `-ini` document has no patterns),
//! copied. The paragraph's list ends with `\penalty10000\parfillskip`
//! (`Exec::end_par` puts them there, tex.web 816); the result is the
//! justified lines, each with what follows it in the vertical list (the
//! marks the packager moved out, the interline penalty) and TeX's
//! diagnostic about its box.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::arith::{AWFUL_BAD, INF_BAD, badness, fract};
use crate::item::{BoxNode, Disc, Interline, Item, Para, is_zero};
use crate::pack::{Params, Spec, end_lr_type, hpack, math_subtype};
use crate::show::{Ctx, Place, report};
use crate::val::Glue;

/// What line breaking reads, read by the `\par` node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineParams {
    pub hsize: i32,
    pub pretolerance: i32,
    pub tolerance: i32,
    pub emergency_stretch: i32,
    pub looseness: i32,
    pub linepenalty: i32,
    pub hyphenpenalty: i32,
    pub exhyphenpenalty: i32,
    pub adjdemerits: i32,
    pub doublehyphendemerits: i32,
    pub finalhyphendemerits: i32,
    pub interlinepenalty: i32,
    pub clubpenalty: i32,
    pub widowpenalty: i32,
    pub brokenpenalty: i32,
    /// e-TeX's `\interlinepenalties`, `\clubpenalties`, `\widowpenalties`
    /// (`\displaywidowpenalties` before a display): empty is TeX's null.
    pub interlinepenalties: Arc<[i32]>,
    pub clubpenalties: Arc<[i32]>,
    pub widowpenalties: Arc<[i32]>,
    /// e-TeX's `\lastlinefit` (etex.web §1575–§1583).
    pub last_line_fit: i32,
    /// `\TeXXeTstate` positive: the lines get the LR nodes of the text
    /// directions open across them (etex.web §1517–§1520).
    pub texxet: bool,
    pub leftskip: Arc<Glue>,
    pub rightskip: Arc<Glue>,
    pub hangindent: i32,
    pub hangafter: i32,
    pub parshape: Arc<Vec<(i32, i32)>>,
    /// The enclosing vertical list's `\prevgraf` (tex.web 877).
    pub prev_graf: i32,
    /// `\hbadness`, `\hfuzz`, `\overfullrule` for the lines.
    pub pack: Params,
    /// What appending each line to the vertical list reads.
    pub inter: Arc<Interline>,
    /// For the diagnostics.
    pub show: Ctx,
    /// What hyphenating the paragraph's words reads (its second pass,
    /// tex.web 891–918).
    pub hyph: Option<Arc<crate::hyph::HyphParams>>,
}

/// A line: its box (shifted by its indentation) and what follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub b: Arc<BoxNode>,
    /// Marks the packager moved out of the line (tex.web 655).
    pub adjust: Vec<Item>,
    /// The interline penalty after it, if any (tex.web 890).
    pub pen: Option<i32>,
    /// TeX's diagnostic about the line's box, a raw log entry.
    pub report: Option<String>,
    /// Where its box's items came from in the paragraph's list: runs
    /// `(item of the box, item of the list)`, each going on one for one
    /// (a hyphenated word's items, a discretionary's, the skips: the
    /// list's item they stand at). With the paragraph's
    /// [`crate::item::Origins`], the node that made each item.
    pub from: Arc<[(u32, u32)]>,
}

impl Line {
    /// The paragraph list's item that the box's item `i` came from.
    pub fn list_item(&self, i: usize) -> Option<usize> {
        let k = self.from.partition_point(|x| x.0 as usize <= i).checked_sub(1)?;
        let (b, l) = self.from[k];
        Some(l as usize + (i - b as usize))
    }

    /// The node (its index in its piece) that made the box's item `i`, the
    /// line being one of `para`'s, held by node `para_node`: SyncTeX's
    /// and the viewer's link from a glyph to its source place
    /// (`Build::source_place`). Inside a box among the items: that box's
    /// [`crate::item::Origins`], back from the node found here.
    pub fn node_of(&self, para: &crate::item::Para, para_node: u32, i: usize) -> Option<u32> {
        para_node.checked_sub(para.origins.of(self.list_item(i)?)?)
    }
}

const EJECT: i32 = -10000;
/// `max_dimen`.
const MAX_DIMEN: i32 = 0o7_777_777_777;
/// `\parfillskip`'s glue subtype (its parameter's number + 1).
const PAR_FILL_SKIP: u8 = 15;
/// `max_halfword`: the line number of the last active node (tex.web 820).
const MAX_HALFWORD: i32 = 0x0FFF_FFFF;

/// Widths: natural, stretch of each order, shrink (tex.web 823).
type W = [i64; 6];

fn add(a: &mut W, b: &W) {
    for k in 0..6 {
        a[k] += b[k];
    }
}

fn sub(a: &mut W, b: &W) {
    for k in 0..6 {
        a[k] -= b[k];
    }
}

fn glue_widths(w: &mut W, g: &Glue) {
    w[0] += i64::from(g.width);
    w[1 + usize::from(g.stretch_order.min(3))] += i64::from(g.stretch);
    // (`check_shrinkage`: an infinite shrink is taken as finite, tex.web 826)
    w[5] += i64::from(g.shrink);
}

/// The widths an item adds (tex.web 866–871).
fn widths(it: &Item) -> W {
    let mut w = [0i64; 6];
    match it {
        Item::Glue { g, .. } => glue_widths(&mut w, g),
        Item::Leaders(l) => glue_widths(&mut w, &l.g),
        Item::Char(..) | Item::Lig(_) | Item::Box(_) | Item::Rule { .. } | Item::Kern { .. } | Item::Math { .. } => {
            w[0] = i64::from(it.width());
        }
        _ => {}
    }
    w
}

fn list_widths(l: &[Item]) -> W {
    let mut w = [0i64; 6];
    for it in l {
        add(&mut w, &widths(it));
    }
    w
}

/// An active node (tex.web 819): the break it stands for (a passive node;
/// `None`, the paragraph's start), and what the line after it is.
#[derive(Clone, Copy)]
struct Active {
    passive: Option<usize>,
    hyphenated: bool,
    line: i32,
    fit: usize,
    total: i64,
    /// e-TeX's `active_short`, `active_glue` (with `\lastlinefit`): the
    /// line's shortfall and its glue's stretch or shrink.
    short: i64,
    glue: i64,
}

/// A passive node (tex.web 821): the break's item (`None`: the end) and
/// the break before it.
#[derive(Clone, Copy)]
struct Passive {
    at: Option<usize>,
    prev: Option<usize>,
}

/// What the pass does at an item (tex.web 866), read from it first.
enum Step {
    Glue,
    Lang(crate::hyph::LangState),
    Kern,
    Math(bool, u8),
    Penalty(i32),
    Disc(Option<W>, u8),
    Other,
}

struct Breaker<'a> {
    /// The paragraph's list: borrowed until its second pass hyphenates a
    /// word in it (tex.web 894); never copied item by item otherwise (an
    /// item's font is an `Arc` every thread shares).
    items: std::borrow::Cow<'a, [Item]>,
    p: &'a LineParams,
    /// `prefix[i]`: the widths of `items[..i]`.
    prefix: Vec<W>,
    background: W,
    easy_line: i32,
    last_special_line: i32,
    first_width: i32,
    second_width: i32,
    first_indent: i32,
    second_indent: i32,
    active: Vec<Active>,
    passive: Vec<Passive>,
    minimal: [i64; 4],
    minimum: i64,
    best_place: [Option<usize>; 4],
    best_line: [i32; 4],
    threshold: i32,
    final_pass: bool,
    /// e-TeX's `do_last_line_fit`, `fill_width`, `best_pl_short`,
    /// `best_pl_glue`.
    last_line_fit: bool,
    fill_width: [i64; 3],
    best_short: [i64; 4],
    best_glue: [i64; 4],
    second_pass: bool,
    /// `cur_lang`, `l_hyf`, `r_hyf` (tex.web 892).
    lang: crate::hyph::LangState,
    /// Hyphen characters the font lacks (tex.web 915), for the log.
    missing: Vec<(Arc<crate::tfm::Font>, i32)>,
    /// For each item of `items`, the paragraph list's item it came from,
    /// once a word was hyphenated (`None`: the same index).
    orig: Option<Vec<u32>>,
}

impl Breaker<'_> {
    /// etex.web §1579: the badness and fitness of the last line, adjusted
    /// as the line before it was (`\lastlinefit`), and the adjustment;
    /// `None` when the criteria fail (the normal algorithm goes on).
    fn last_line(&self, a: &Active, w: &W, shortfall: i64) -> Option<(i32, usize, i64)> {
        if a.short == 0 || a.glue <= 0 {
            return None;
        }
        if w[2] != self.fill_width[0] || w[3] != self.fill_width[1] || w[4] != self.fill_width[2] {
            return None;
        }
        let g = if a.short > 0 { w[1] } else { w[5] };
        if g <= 0 {
            return None;
        }
        let clamp = |x: i64| x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        let mut err = false;
        let mut g = fract(clamp(g), clamp(a.short), clamp(a.glue), MAX_DIMEN).unwrap_or_else(|| {
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
            g = if a.short > 0 { MAX_DIMEN } else { -MAX_DIMEN };
        }
        let mut g = i64::from(g);
        match g.cmp(&0) {
            std::cmp::Ordering::Greater => {
                // (§1580)
                if g > shortfall {
                    g = shortfall;
                }
                if g > 7_230_584 && w[1] < 1_663_497 {
                    return Some((INF_BAD, 0, g));
                }
                let b = badness(g as i32, w[1] as i32);
                let fit = if b > 99 {
                    0
                } else if b > 12 {
                    1
                } else {
                    2
                };
                Some((b, fit, g))
            }
            std::cmp::Ordering::Less => {
                // (§1581)
                if -g > w[5] {
                    g = -w[5];
                }
                let b = badness((-g) as i32, w[5] as i32);
                Some((b, if b > 12 { 3 } else { 2 }, g))
            }
            std::cmp::Ordering::Equal => None,
        }
    }

    /// Where the line after an active node's break begins, and the widths
    /// before that point it has (a discretionary's post-break list): what
    /// tex.web's `break_width` computes (tex.web 837–840).
    fn start(&self, a: &Active) -> (usize, W) {
        let Some(pi) = a.passive else { return (0, [0; 6]) };
        let Some(b) = self.passive[pi].at else { return (self.items.len(), [0; 6]) };
        let mut s = b;
        if let Item::Disc(d) = &self.items[b] {
            let v = b + usize::from(d.replace);
            if !d.post.is_empty() {
                return (v + 1, list_widths(&d.post));
            }
            s = v + 1;
        }
        while s < self.items.len() {
            match &self.items[s] {
                Item::Glue { .. } | Item::Leaders(_) | Item::Penalty(_) | Item::Kern { sub: 1, .. } | Item::Math { .. } => {}
                _ => break,
            }
            s += 1;
        }
        (s, [0; 6])
    }

    /// §850: the length of line `l`.
    fn line_width(&self, l: i32) -> i32 {
        if l > self.last_special_line {
            self.second_width
        } else if self.p.parshape.is_empty() {
            self.first_width
        } else {
            self.p.parshape[(l - 1) as usize].1
        }
    }

    /// §829: `try_break` at item `cur` (`None`: the paragraph's end), with
    /// the widths of a discretionary's pre-break list.
    fn try_break(&mut self, mut pi: i32, hyphenated: bool, cur: Option<usize>, pre: &W) {
        // (§831)
        if pi.abs() >= 10000 {
            if pi > 0 {
                return;
            }
            pi = EJECT;
        }
        let upto = cur.unwrap_or(self.items.len());
        let mut old_l = 0;
        let mut line_width = 0;
        let mut i = 0usize;
        loop {
            // (§835: a line number class has ended)
            let l = self.active.get(i).map_or(MAX_HALFWORD, |a| a.line);
            if l > old_l {
                if self.minimum < i64::from(AWFUL_BAD) && (old_l != self.easy_line || i == self.active.len()) {
                    // (§836: new active nodes for the best feasible breaks)
                    let adj = i64::from(self.p.adjdemerits.abs());
                    if adj >= i64::from(AWFUL_BAD) - self.minimum {
                        self.minimum = i64::from(AWFUL_BAD) - 1;
                    } else {
                        self.minimum += adj;
                    }
                    for fit in 0..4 {
                        if self.minimal[fit] <= self.minimum {
                            // (§845)
                            self.passive.push(Passive { at: cur, prev: self.best_place[fit] });
                            let a = Active {
                                passive: Some(self.passive.len() - 1),
                                hyphenated,
                                line: self.best_line[fit] + 1,
                                fit,
                                total: self.minimal[fit],
                                short: self.best_short[fit],
                                glue: self.best_glue[fit],
                            };
                            self.active.insert(i, a);
                            i += 1;
                        }
                        self.minimal[fit] = i64::from(AWFUL_BAD);
                    }
                    self.minimum = i64::from(AWFUL_BAD);
                }
                if i == self.active.len() {
                    return;
                }
                // (§850)
                if l > self.easy_line {
                    line_width = self.second_width;
                    old_l = MAX_HALFWORD - 1;
                } else {
                    old_l = l;
                    line_width = self.line_width(l);
                }
            }
            // (§851: the demerits for a line from `a` to `cur`)
            let a = self.active[i];
            let (s, extra) = self.start(&a);
            let mut w = self.background;
            add(&mut w, &self.prefix[upto]);
            sub(&mut w, &self.prefix[s]);
            add(&mut w, &extra);
            add(&mut w, pre);
            let mut shortfall = i64::from(line_width) - w[0];
            // (e-TeX's `g`: the line's glue stretch or shrink, or the last
            // line's adjustment; set by `last_line`: `found`)
            let mut g: i64 = 0;
            let mut found = false;
            let (b, fit): (i32, usize) = if shortfall > 0 {
                // (§852)
                if w[2] != 0 || w[3] != 0 || w[4] != 0 {
                    let mut r = (0, 2);
                    if self.last_line_fit {
                        // (etex.web §1579: the paragraph's last line)
                        if let Some((b, fit, x)) = cur.map_or_else(|| self.last_line(&a, &w, shortfall), |_| None) {
                            r = (b, fit);
                            g = x;
                            found = true;
                        } else {
                            shortfall = 0;
                        }
                    }
                    r
                } else if shortfall > 7_230_584 && w[1] < 1_663_497 {
                    (INF_BAD, 0)
                } else {
                    let b = badness(shortfall as i32, w[1] as i32);
                    let fit = if b > 99 {
                        0
                    } else if b > 12 {
                        1
                    } else {
                        2
                    };
                    (b, fit)
                }
            } else {
                // (§853)
                let b = if -shortfall > w[5] { INF_BAD + 1 } else { badness((-shortfall) as i32, w[5] as i32) };
                (b, if b > 12 { 3 } else { 2 })
            };
            if self.last_line_fit && !found {
                // (etex.web §1582)
                if cur.is_none() {
                    shortfall = 0;
                }
                g = match shortfall.cmp(&0) {
                    std::cmp::Ordering::Greater => w[1],
                    std::cmp::Ordering::Less => w[5],
                    std::cmp::Ordering::Equal => 0,
                };
            }
            let mut artificial = false;
            let stays = if b > INF_BAD || pi == EJECT {
                // (§854)
                if self.final_pass && self.minimum == i64::from(AWFUL_BAD) && self.active.len() == 1 && i == 0 {
                    artificial = true;
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
            // (§855: record a new feasible break)
            let mut d: i64 = if artificial {
                0
            } else {
                // (§859)
                let mut d = i64::from(self.p.linepenalty) + i64::from(b);
                d = if d.abs() >= 10000 { 100_000_000 } else { d * d };
                if pi != 0 {
                    if pi > 0 {
                        d += i64::from(pi) * i64::from(pi);
                    } else if pi > EJECT {
                        d -= i64::from(pi) * i64::from(pi);
                    }
                }
                if hyphenated && a.hyphenated {
                    if cur.is_some() {
                        d += i64::from(self.p.doublehyphendemerits);
                    } else {
                        d += i64::from(self.p.finalhyphendemerits);
                    }
                }
                if fit.abs_diff(a.fit) > 1 {
                    d += i64::from(self.p.adjdemerits);
                }
                d
            };
            d += a.total;
            if d <= self.minimal[fit] {
                self.minimal[fit] = d;
                self.best_place[fit] = a.passive;
                self.best_line[fit] = l;
                if self.last_line_fit {
                    // (etex.web §1583)
                    self.best_short[fit] = shortfall;
                    self.best_glue[fit] = g;
                }
                if d < self.minimum {
                    self.minimum = d;
                }
            }
            if stays {
                i += 1;
            } else {
                self.active.remove(i);
            }
        }
    }

    /// One pass (tex.web 863–873): the chosen final active node, if the
    /// pass found breaks.
    fn pass(&mut self) -> Option<Active> {
        self.active = vec![Active {
            passive: None,
            hyphenated: false,
            line: self.p.prev_graf + 1,
            fit: 2,
            total: 0,
            short: 0,
            glue: 0,
        }];
        self.passive.clear();
        self.minimal = [i64::from(AWFUL_BAD); 4];
        self.minimum = i64::from(AWFUL_BAD);
        let mut k = 0;
        let none = [0i64; 6];
        // (§866: no break at glue inside a formula)
        let mut auto_breaking = true;
        // (§891: initialize for hyphenating a paragraph)
        if self.second_pass
            && let Some(h) = &self.p.hyph
        {
            self.lang = h.init;
        }
        // (no hyphenation at all when no language present has patterns or
        // exceptions)
        let hyph = self.p.hyph.as_deref().filter(|h| h.tables.iter().any(|(_, a, b)| !a.is_empty() || !b.is_empty()));
        // (§866: `prev_p`, the node before the current one as the loop goes:
        // a discretionary's replaced nodes are passed over)
        let mut prev: Option<usize> = None;
        while k < self.items.len() && !self.active.is_empty() {
            // (§866: what the item needs, read before the breaker changes)
            let step = match &self.items[k] {
                Item::Glue { .. } | Item::Leaders(_) => Step::Glue,
                Item::Lang { lang, lhm, rhm } => Step::Lang(crate::hyph::LangState { lang: *lang, lhm: *lhm, rhm: *rhm }),
                Item::Kern { sub: 1, .. } => Step::Kern,
                Item::Math { after, lr, .. } => Step::Math(*after, *lr),
                Item::Penalty(p) => Step::Penalty(*p),
                Item::Disc(d) => Step::Disc((!d.pre.is_empty()).then(|| list_widths(&d.pre)), d.replace),
                _ => Step::Other,
            };
            match step {
                Step::Glue => {
                    // (§868)
                    if let Some(pk) = prev.filter(|_| auto_breaking) {
                        let pr = &self.items[pk];
                        if pr.is_char() || pr.precedes_break() || matches!(pr, Item::Kern { sub, .. } if *sub != 1) {
                            self.try_break(0, false, Some(k), &none);
                        }
                    }
                    // (§894: try to hyphenate the following word)
                    if self.second_pass
                        && auto_breaking
                        && let Some(h) = hyph
                        && let Some(w) = crate::hyph::hyphenate_after(&self.items, k, &mut self.lang, h)
                    {
                        self.missing.extend(w.missing);
                        let o = self.orig.get_or_insert_with(|| (0..self.items.len() as u32).collect());
                        let at = o[w.start];
                        o.splice(w.start..w.end, std::iter::repeat_n(at, w.items.len()));
                        self.items.to_mut().splice(w.start..w.end, w.items);
                        self.reprefix(w.start);
                    }
                }
                Step::Lang(l) => {
                    // (§1362)
                    self.lang = l;
                }
                Step::Kern => {
                    if auto_breaking && self.items.get(k + 1).is_some_and(Item::is_glue) {
                        self.try_break(0, false, Some(k), &none);
                    }
                }
                Step::Math(after, lr) => {
                    // (etex.web: TeXXeT's nodes leave it as it is)
                    if lr < 4 {
                        auto_breaking = after;
                    }
                    if auto_breaking && self.items.get(k + 1).is_some_and(Item::is_glue) {
                        self.try_break(0, false, Some(k), &none);
                    }
                }
                Step::Penalty(p) => self.try_break(p, false, Some(k), &none),
                Step::Disc(pre, replace) => {
                    // (§869)
                    match pre {
                        None => self.try_break(self.p.exhyphenpenalty, true, Some(k), &none),
                        Some(pre) => self.try_break(self.p.hyphenpenalty, true, Some(k), &pre),
                    }
                    prev = Some(k);
                    k += usize::from(replace) + 1;
                    continue;
                }
                Step::Other => {}
            }
            prev = Some(k);
            k += 1;
        }
        if k < self.items.len() || self.active.is_empty() {
            return None;
        }
        // (§873: the final break)
        self.try_break(EJECT, true, None, &none);
        if self.active.is_empty() {
            return None;
        }
        // (§874: fewest demerits)
        let mut best: Option<Active> = None;
        let mut fewest = i64::from(AWFUL_BAD);
        for a in &self.active {
            if a.total < fewest {
                fewest = a.total;
                best = Some(*a);
            }
        }
        let best = best?;
        if self.p.looseness == 0 {
            return Some(best);
        }
        // (§875: the desired looseness)
        let best_line = best.line;
        let mut bet = best;
        let mut actual = 0;
        for a in &self.active {
            let diff = a.line - best_line;
            if (diff < actual && self.p.looseness <= diff) || (diff > actual && self.p.looseness >= diff) {
                bet = *a;
                actual = diff;
                fewest = a.total;
            } else if diff == actual && a.total < fewest {
                bet = *a;
                fewest = a.total;
            }
        }
        if actual == self.p.looseness || self.final_pass { Some(bet) } else { None }
    }
}

impl Breaker<'_> {
    /// The widths' prefix sums again from item `from` on (a word there
    /// was hyphenated).
    fn reprefix(&mut self, from: usize) {
        self.prefix.truncate(from + 1);
        let mut acc = self.prefix[from];
        for it in &self.items[from..] {
            add(&mut acc, &widths(it));
            self.prefix.push(acc);
        }
    }
}

/// Breaks a paragraph into lines (none for a carrier of vertical items).
pub fn break_lines(para: &Para) -> Vec<Arc<Line>> {
    let Some(p) = &para.lb else { return Vec::new() };
    lines(&para.hlist, p)
}

/// tex.web 826, `finite_shrink`: glue whose shrinkability is infinite,
/// with that shrinkability made finite (`None`: it is finite).
fn finite_shrink(g: &Glue) -> Option<Arc<Glue>> {
    (g.shrink_order != 0 && g.shrink != 0).then(|| Arc::new(Glue { shrink_order: 0, ..*g }))
}

/// The lines of a paragraph's list.
pub fn lines(items: &[Item], p: &LineParams) -> Vec<Arc<Line>> {
    // (§826–§827, §868: `check_shrinkage` makes the shrinkability of
    // `\leftskip`, `\rightskip` and of the list's glue finite — the glue
    // the lines are made of too)
    let skips = (finite_shrink(&p.leftskip), finite_shrink(&p.rightskip));
    let fixed;
    let p = if skips.0.is_some() || skips.1.is_some() {
        fixed = LineParams {
            leftskip: skips.0.unwrap_or_else(|| p.leftskip.clone()),
            rightskip: skips.1.unwrap_or_else(|| p.rightskip.clone()),
            ..p.clone()
        };
        &fixed
    } else {
        p
    };
    let owned: Vec<Item>;
    let items = if items.iter().any(|it| it.glue().is_some_and(|g| finite_shrink(g).is_some())) {
        owned = items
            .iter()
            .map(|it| match it {
                Item::Glue { g, sub, .. } => match finite_shrink(g) {
                    Some(f) => Item::Glue { g: f, sub: *sub, zero: false },
                    None => it.clone(),
                },
                Item::Leaders(l) => match finite_shrink(&l.g) {
                    Some(f) => Item::Leaders(Arc::new(crate::item::Leaders { g: f, zero: false, ..(**l).clone() })),
                    None => it.clone(),
                },
                _ => it.clone(),
            })
            .collect();
        &owned[..]
    } else {
        items
    };
    let mut prefix = Vec::with_capacity(items.len() + 1);
    let mut acc = [0i64; 6];
    prefix.push(acc);
    for it in items {
        add(&mut acc, &widths(it));
        prefix.push(acc);
    }
    // (§827: the background)
    let mut background = [0i64; 6];
    glue_widths(&mut background, &p.leftskip);
    glue_widths(&mut background, &p.rightskip);
    // (§848, §849: the lines' lengths)
    let (mut lsl, mut fw, mut fi, mut sw, mut si) = (0, p.hsize, 0, p.hsize, 0);
    if p.parshape.is_empty() {
        if p.hangindent != 0 {
            lsl = p.hangafter.abs();
            if p.hangafter < 0 {
                fw = p.hsize - p.hangindent.abs();
                fi = p.hangindent.max(0);
            } else {
                sw = p.hsize - p.hangindent.abs();
                si = p.hangindent.max(0);
            }
        }
    } else {
        lsl = p.parshape.len() as i32 - 1;
        let last = p.parshape[p.parshape.len() - 1];
        sw = last.1;
        si = last.0;
    }
    let easy_line = if p.looseness == 0 { lsl } else { MAX_HALFWORD };
    let mut br = Breaker {
        orig: None,
        items: std::borrow::Cow::Borrowed(items),
        p,
        prefix,
        background,
        easy_line,
        last_special_line: lsl,
        first_width: fw,
        second_width: sw,
        first_indent: fi,
        second_indent: si,
        active: Vec::new(),
        passive: Vec::new(),
        minimal: [i64::from(AWFUL_BAD); 4],
        minimum: i64::from(AWFUL_BAD),
        best_place: [None; 4],
        best_line: [0; 4],
        threshold: 0,
        final_pass: false,
        last_line_fit: false,
        fill_width: [0; 3],
        best_short: [0; 4],
        best_glue: [0; 4],
        second_pass: false,
        lang: p.hyph.as_ref().map(|h| h.init).unwrap_or_default(),
        missing: Vec::new(),
    };
    // (etex.web §1578: `\lastlinefit` needs `\parfillskip`'s stretch
    // infinite and `\leftskip` plus `\rightskip`'s finite)
    let fill = match items.last() {
        Some(Item::Glue { g, sub: PAR_FILL_SKIP, .. }) => Some(g.clone()),
        _ => None,
    };
    if p.last_line_fit > 0
        && let Some(q) = &fill
        && q.stretch > 0
        && q.stretch_order > 0
        && br.background[2] == 0
        && br.background[3] == 0
        && br.background[4] == 0
    {
        br.last_line_fit = true;
        br.fill_width[usize::from(q.stretch_order.min(3)) - 1] = i64::from(q.stretch);
    }
    // (§863: the passes)
    let mut second = if p.pretolerance >= 0 {
        br.threshold = p.pretolerance;
        false
    } else {
        br.threshold = p.tolerance;
        br.final_pass = p.emergency_stretch <= 0;
        true
    };
    let best = loop {
        if br.threshold > INF_BAD {
            br.threshold = INF_BAD;
        }
        br.second_pass = second;
        if let Some(b) = br.pass() {
            break b;
        }
        if second {
            br.background[1] += i64::from(p.emergency_stretch);
            br.final_pass = true;
        } else {
            br.threshold = p.tolerance;
            second = true;
            br.final_pass = p.emergency_stretch <= 0;
        }
    };
    // (etex.web §1583: the last line's `\parfillskip` adjusted)
    if br.last_line_fit
        && best.short != 0
        && let Some(q) = &fill
    {
        let mut g = **q;
        g.width = (i64::from(g.width) + best.short - best.glue) as i32;
        g.stretch = 0;
        let last = br.items.len() - 1;
        br.items.to_mut()[last] = Item::Glue { g: Arc::new(g), sub: PAR_FILL_SKIP, zero: false };
    }
    let mut out = post_line_break(&br, best);
    // (a hyphen character the font lacks, reported as the second pass met
    // it: before the lines' diagnostics)
    if p.hyph.as_ref().is_some_and(|h| h.lost_chars) && !br.missing.is_empty() {
        let mut s = String::from(crate::show::RAW);
        for (f, c) in &br.missing {
            let ch = char::from_u32(*c as u32).unwrap_or('?');
            s.push(crate::show::NL);
            let _ = write!(s, "Missing character: There is no {ch} in font {}!", f.name);
            s.push(crate::show::NL);
        }
        if let Some(first) = out.first_mut() {
            let l = Arc::make_mut(first);
            if let Some(r) = &l.report {
                s.push_str(r.strip_prefix(crate::show::RAW).unwrap_or(r));
            }
            l.report = Some(s);
        }
    }
    out
}

/// §877–§890: the lines, justified.
fn post_line_break(br: &Breaker<'_>, best: Active) -> Vec<Arc<Line>> {
    let p = br.p;
    // (etex.web's LR stack, a local: the end types of the text directions
    // open, innermost last)
    let mut lr: Vec<u8> = Vec::new();
    let adjust_lr = |lr: &mut Vec<u8>, it: &Item| {
        if let Item::Math { after, lr: base, .. } = it {
            let s = math_subtype(*base, *after);
            if *after {
                if lr.last() == Some(&end_lr_type(s)) {
                    lr.pop();
                }
            } else {
                lr.push(end_lr_type(s));
            }
        }
    };
    let items = &br.items[..];
    let mut breaks: Vec<Option<usize>> = Vec::new();
    let mut q = best.passive;
    while let Some(pi) = q {
        breaks.push(br.passive[pi].at);
        q = br.passive[pi].prev;
    }
    breaks.reverse();
    let best_line = best.line;
    let mut out = Vec::with_capacity(breaks.len());
    let mut start = 0usize;
    // (a discretionary's post-break list, beginning the next line)
    let mut carry: Vec<Item> = Vec::new();
    let orig = |i: usize| br.orig.as_ref().map_or(i as u32, |o| o.get(i).copied().unwrap_or(i as u32));
    for (bi, &brk) in breaks.iter().enumerate() {
        let cur_line = p.prev_graf + 1 + bi as i32;
        let mut line: Vec<Item> = Vec::new();
        // (the list's item each of the line's items came from)
        let mut from: Vec<u32> = Vec::new();
        let at_start = orig(start.min(items.len().saturating_sub(1)));
        if p.texxet {
            // (etex.web §1517: the directions open at the line's start begin
            // it, outermost first; then those of the line's own nodes)
            for &e in &lr {
                line.push(Item::Math { w: 0, after: false, lr: e - 1 });
            }
            for it in &items[start..brk.unwrap_or(items.len()).max(start)] {
                adjust_lr(&mut lr, it);
            }
        }
        from.resize(line.len(), at_start);
        line.append(&mut carry);
        from.resize(line.len(), at_start);
        let mut disc_break = false;
        let mut post_disc = false;
        let rightskip = Item::Glue { g: p.rightskip.clone(), sub: 9, zero: is_zero(&p.rightskip) };
        // (§881: the end of the line, with `\rightskip`)
        let next_start = if let Some(b) = brk {
            match &items[b] {
                Item::Glue { .. } | Item::Leaders(_) => {
                    line.extend(items[start..b].iter().cloned());
                    line.push(rightskip);
                    b + 1
                }
                Item::Disc(d) => {
                    // (§882: the discretionary becomes compulsory)
                    line.extend(items[start..b].iter().cloned());
                    line.push(Item::Disc(Arc::new(Disc { pre: Vec::new(), post: Vec::new(), replace: 0 })));
                    line.extend(d.pre.iter().cloned());
                    line.push(rightskip);
                    if !d.post.is_empty() {
                        carry.clone_from(&d.post);
                        post_disc = true;
                    }
                    disc_break = true;
                    b + 1 + usize::from(d.replace)
                }
                Item::Kern { sub, .. } => {
                    line.extend(items[start..b].iter().cloned());
                    line.push(Item::Kern { w: 0, sub: *sub });
                    line.push(rightskip);
                    b + 1
                }
                Item::Math { after, lr: base, .. } => {
                    line.extend(items[start..b].iter().cloned());
                    let m = Item::Math { w: 0, after: *after, lr: *base };
                    if p.texxet {
                        adjust_lr(&mut lr, &m);
                    }
                    line.push(m);
                    line.push(rightskip);
                    b + 1
                }
                _ => {
                    line.extend(items[start..=b].iter().cloned());
                    line.push(rightskip);
                    b + 1
                }
            }
        } else {
            line.extend(items[start..].iter().cloned());
            line.push(rightskip);
            items.len()
        };
        // (the line's own items, one for one; what the break added, at the
        // break)
        {
            let own_end = brk.map_or(items.len(), |b| if matches!(items[b], Item::Glue { .. } | Item::Leaders(_) | Item::Disc(_) | Item::Kern { .. } | Item::Math { .. }) { b } else { b + 1 });
            let at_break = orig(brk.unwrap_or(items.len().saturating_sub(1)).min(items.len().saturating_sub(1)));
            for i in start..own_end.max(start) {
                from.push(orig(i));
            }
            from.resize(line.len(), at_break);
        }
        if p.texxet && !lr.is_empty() {
            // (etex.web §1520: and end before `\rightskip`, innermost first)
            let at = line.len() - 1;
            let ends: Vec<Item> = lr.iter().rev().map(|&e| Item::Math { w: 0, after: true, lr: e - 1 }).collect();
            let fill = from.get(at).copied().unwrap_or(at_start);
            from.splice(at..at, std::iter::repeat_n(fill, ends.len()));
            line.splice(at..at, ends);
        }
        // (§887: `\leftskip`, unless zero)
        if !is_zero(&p.leftskip) {
            line.insert(0, Item::Glue { g: p.leftskip.clone(), sub: 8, zero: false });
            from.insert(0, at_start);
        }
        // (the packager moves the marks, insertions and adjustments out)
        let from: Vec<u32> = line
            .iter()
            .zip(&from)
            .filter(|(i, _)| !matches!(i, Item::Mark(_) | Item::Marks(..) | Item::Ins(_) | Item::Adjust(_)))
            .map(|(_, &f)| f)
            .collect();
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (j, &f) in from.iter().enumerate() {
            match runs.last() {
                Some(&(b, l)) if l as usize + (j - b as usize) == f as usize => {}
                _ => runs.push((j as u32, f)),
            }
        }
        // (§889: the packaging)
        let (width, indent) = if cur_line > br.last_special_line {
            (br.second_width, br.second_indent)
        } else if p.parshape.is_empty() {
            (br.first_width, br.first_indent)
        } else {
            let (i, w) = p.parshape[(cur_line - 1) as usize];
            (w, i)
        };
        let mut adjust = Vec::new();
        if p.texxet {
            crate::pack::lr_fix(&mut line);
        }
        let packed = hpack(line, Spec::Exactly(width), &p.pack, Some(&mut adjust));
        // (the diagnostic is hpack's: before the shift, tex.web 889)
        let report = packed.report.map(|r| report(r, &packed.b, Place::Paragraph, &p.show));
        let mut b = packed.b;
        b.shift = indent;
        // (§890: the interline penalty)
        let mut pen = None;
        if cur_line + 1 != best_line {
            // (etex.web §890: a penalties array, where there is one)
            let at = |a: &[i32], r: i32| a[(r.max(1) as usize).min(a.len()) - 1];
            let mut x = if p.interlinepenalties.is_empty() {
                p.interlinepenalty
            } else {
                at(&p.interlinepenalties, cur_line)
            };
            if !p.clubpenalties.is_empty() {
                x += at(&p.clubpenalties, cur_line - p.prev_graf);
            } else if cur_line == p.prev_graf + 1 {
                x += p.clubpenalty;
            }
            if !p.widowpenalties.is_empty() {
                x += at(&p.widowpenalties, best_line - cur_line - 1);
            } else if cur_line + 2 == best_line {
                x += p.widowpenalty;
            }
            if disc_break {
                x += p.brokenpenalty;
            }
            if x != 0 {
                pen = Some(x);
            }
        }
        out.push(Arc::new(Line { b: Arc::new(b), adjust, pen, report, from: Arc::from(runs) }));
        // (§879: discardable items at the next line's start go)
        start = next_start;
        if !post_disc {
            let next_break = breaks.get(bi + 1).copied().flatten();
            while start < items.len() && Some(start) != next_break {
                let it = &items[start];
                if it.non_discardable() || matches!(it, Item::Kern { sub, .. } if *sub != 1) {
                    break;
                }
                if p.texxet {
                    // (etex.web §879: a math node discarded still counts)
                    adjust_lr(&mut lr, it);
                }
                start += 1;
            }
        }
    }
    out
}

/// `\prevgraf` after a paragraph of `n` lines (tex.web 890).
pub fn prev_graf_after(p: &LineParams, n: usize) -> i32 {
    p.prev_graf + n as i32
}
