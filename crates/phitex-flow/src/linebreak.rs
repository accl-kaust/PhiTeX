//! Line breaking: a pure function of a paragraph (tex.web parts 38–39,
//! §813–§890, without hyphenation: an `-ini` document has no patterns),
//! copied. The paragraph's list ends with `\penalty10000\parfillskip`
//! (`Exec::end_par` puts them there, tex.web 816); the result is the
//! justified lines, each with what follows it in the vertical list (the
//! marks the packager moved out, the interline penalty) and TeX's
//! diagnostic about its box.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::arith::{AWFUL_BAD, INF_BAD, badness};
use crate::item::{BoxNode, Disc, Interline, Item, Para, is_zero};
use crate::pack::{Params, Spec, hpack};
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
}

const EJECT: i32 = -10000;
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
}

/// A passive node (tex.web 821): the break's item (`None`: the end) and
/// the break before it.
#[derive(Clone, Copy)]
struct Passive {
    at: Option<usize>,
    prev: Option<usize>,
}

struct Breaker<'a> {
    /// The paragraph's list: owned, since its second pass hyphenates
    /// words in it (tex.web 894).
    items: Vec<Item>,
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
    second_pass: bool,
    /// `cur_lang`, `l_hyf`, `r_hyf` (tex.web 892).
    lang: crate::hyph::LangState,
    /// Hyphen characters the font lacks (tex.web 915), for the log.
    missing: Vec<(Arc<crate::tfm::Font>, i32)>,
}

impl Breaker<'_> {
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
                Item::Glue { .. } | Item::Penalty(_) | Item::Kern { sub: 1, .. } | Item::Math { .. } => {}
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
            let shortfall = i64::from(line_width) - w[0];
            let (b, fit): (i32, usize) = if shortfall > 0 {
                // (§852)
                if w[2] != 0 || w[3] != 0 || w[4] != 0 {
                    (0, 2)
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
        self.active = vec![Active { passive: None, hyphenated: false, line: self.p.prev_graf + 1, fit: 2, total: 0 }];
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
        while k < self.items.len() && !self.active.is_empty() {
            // (§866)
            let it = self.items[k].clone();
            match &it {
                Item::Glue { .. } => {
                    // (§868)
                    if k > 0 && auto_breaking {
                        let pr = &self.items[k - 1];
                        if pr.is_char() || pr.precedes_break() || matches!(pr, Item::Kern { sub, .. } if *sub != 1) {
                            self.try_break(0, false, Some(k), &none);
                        }
                    }
                    // (§894: try to hyphenate the following word)
                    if self.second_pass
                        && auto_breaking
                        && let Some(h) = self.p.hyph.clone()
                        && let Some(w) = crate::hyph::hyphenate_after(&self.items, k, &mut self.lang, &h)
                    {
                        self.missing.extend(w.missing);
                        self.items.splice(w.start..w.end, w.items);
                        self.reprefix(w.start);
                    }
                }
                Item::Lang { lang, lhm, rhm } => {
                    // (§1362)
                    self.lang = crate::hyph::LangState { lang: *lang, lhm: *lhm, rhm: *rhm };
                }
                Item::Kern { sub: 1, .. } => {
                    if auto_breaking && matches!(self.items.get(k + 1), Some(Item::Glue { .. })) {
                        self.try_break(0, false, Some(k), &none);
                    }
                }
                Item::Math { after, .. } => {
                    auto_breaking = *after;
                    if auto_breaking && matches!(self.items.get(k + 1), Some(Item::Glue { .. })) {
                        self.try_break(0, false, Some(k), &none);
                    }
                }
                Item::Penalty(p) => self.try_break(*p, false, Some(k), &none),
                Item::Disc(d) => {
                    // (§869)
                    let d = d.clone();
                    if d.pre.is_empty() {
                        self.try_break(self.p.exhyphenpenalty, true, Some(k), &none);
                    } else {
                        let pre = list_widths(&d.pre);
                        self.try_break(self.p.hyphenpenalty, true, Some(k), &pre);
                    }
                    k += usize::from(d.replace);
                }
                _ => {}
            }
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

/// The lines of a paragraph's list.
pub fn lines(items: &[Item], p: &LineParams) -> Vec<Arc<Line>> {
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
        items: items.to_vec(),
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
        second_pass: false,
        lang: p.hyph.as_ref().map(|h| h.init).unwrap_or_default(),
        missing: Vec::new(),
    };
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
    for (bi, &brk) in breaks.iter().enumerate() {
        let cur_line = p.prev_graf + 1 + bi as i32;
        let mut line: Vec<Item> = std::mem::take(&mut carry);
        let mut disc_break = false;
        let mut post_disc = false;
        let rightskip = Item::Glue { g: p.rightskip.clone(), sub: 9, zero: is_zero(&p.rightskip) };
        // (§881: the end of the line, with `\rightskip`)
        let next_start = if let Some(b) = brk {
            match &items[b] {
                Item::Glue { .. } => {
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
                Item::Math { after, .. } => {
                    line.extend(items[start..b].iter().cloned());
                    line.push(Item::Math { w: 0, after: *after });
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
        // (§887: `\leftskip`, unless zero)
        if !is_zero(&p.leftskip) {
            line.insert(0, Item::Glue { g: p.leftskip.clone(), sub: 8, zero: false });
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
        let packed = hpack(line, Spec::Exactly(width), &p.pack, Some(&mut adjust));
        // (the diagnostic is hpack's: before the shift, tex.web 889)
        let report = packed.report.map(|r| report(r, &packed.b, Place::Paragraph, &p.show));
        let mut b = packed.b;
        b.shift = indent;
        // (§890: the interline penalty)
        let mut pen = None;
        if cur_line + 1 != best_line {
            let mut x = p.interlinepenalty;
            if cur_line == p.prev_graf + 1 {
                x += p.clubpenalty;
            }
            if cur_line + 2 == best_line {
                x += p.widowpenalty;
            }
            if disc_break {
                x += p.brokenpenalty;
            }
            if x != 0 {
                pen = Some(x);
            }
        }
        out.push(Arc::new(Line { b: Arc::new(b), adjust, pen, report }));
        // (§879: discardable items at the next line's start go)
        start = next_start;
        if !post_disc {
            let next_break = breaks.get(bi + 1).copied().flatten();
            while start < items.len() && Some(start) != next_break {
                let it = &items[start];
                if it.non_discardable() || matches!(it, Item::Kern { sub, .. } if *sub != 1) {
                    break;
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
