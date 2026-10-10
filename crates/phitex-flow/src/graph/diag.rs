//! `\showbox` and `\showlists` (tex.web 1293–1298, 218–219, 986, 690–698):
//! what they print, a raw log entry, and the "OK" error after it with its
//! context (tex.web 311–318: the current line, read so far and the rest).
//!
//! The lists `\showlists` shows are the open lists at the node (each a
//! level's items, gathered as their gatherers gather them); where each
//! began is a line placeholder the log's gatherer fills (`G` k: the `Open`
//! node of the `k`-th enclosing group; `Q` level: the paragraph's first
//! node at that level). The main vertical list shows what this piece
//! contributed since the page builder was last called; the current page
//! is the page fold's and is not shown (CORE.md: exact where the page is
//! empty).

use std::fmt::Write as _;
use std::sync::Arc;

use super::material::{box_start, push_horizontal};
use super::{Exec, GroupKind, Mode};
use crate::item::Item;
use crate::mlist::{self, MItem};
use crate::page::{IGNORE_DEPTH, push_vertical};
use crate::show::{LINE, LN, NL, RAW, show_box, show_list};
use crate::val::{Key, Val};

/// Where the reader is on the line, in a context placeholder.
pub const CUT: char = '\u{5}';

/// One open list, as `show_activities` prints it.
struct Level {
    mode: Mode,
    inner: bool,
    /// Where it began: a placeholder's code.
    line: String,
    items: Vec<Item>,
    /// `\spacefactor` or `\prevdepth`, `\prevgraf`, the language state.
    aux: i32,
    prev_graf: i32,
    hyph_lang: [u8; 4],
    /// A math list's incomplete fraction (`\over` seen).
    incompleat: Option<mlist::Noad>,
}

impl Exec<'_> {
    /// `\show`, `\showbox`, `\showthe`, `\showlists` (tex.web 1293).
    pub(super) fn show_cmd(&mut self, c: u8) {
        if c == 0 || c == 2 {
            self.show_meaning_or_the(c == 0);
            return;
        }
        let mut s = String::from(RAW);
        if c == 1 {
            // (§1296)
            let n = self.scan_eight_bit_int();
            let _ = write!(s, "{NL}> \\box{n}=");
            match self.get(&Key::Box(n)) {
                Val::Box(b) => {
                    let cx = self.show_ctx(std::slice::from_ref(&Item::Box(b.clone())));
                    show_box(&mut s, &b, &cx);
                }
                _ => s.push_str("void"),
            }
        } else {
            self.show_activities(&mut s);
        }
        // (§1298: `end_diagnostic(true)`, the "OK" error and its context)
        s.push(NL);
        s.push(LN);
        s.push(NL);
        s.push_str("! OK.");
        self.show_context(&mut s);
        s.push(LN);
        s.push(LN);
        self.log_raw(&s);
    }

    /// §1294, §1297: `\show` (a token's meaning) and `\showthe` (a
    /// quantity's value), then §1293's `common_ending`: an error without
    /// help, not counted, in the modes that do not stop.
    fn show_meaning_or_the(&mut self, meaning: bool) {
        let mut head = String::new();
        if meaning {
            let Some(t) = self.next_raw() else { return };
            let m = if self.noexp && super::expand::expands(&self.tok_meaning(&t)) {
                crate::val::Meaning::Prim(crate::val::Prim::Relax)
            } else {
                self.tok_meaning(&t)
            };
            match &t {
                crate::val::Tok::Cs(n) => {
                    let n = self.env.cs().name(*n);
                    self.sprint_cs(&mut head, &n);
                    head.push('=');
                }
                crate::val::Tok::Active(c) => {
                    head.push(*c);
                    head.push('=');
                }
                _ => {}
            }
            self.meaning_text(&mut head, &m);
        } else {
            let toks = self.the_toks();
            self.show_toks(&mut head, &toks);
        }
        let cur = self.selector();
        let mut s = String::from(RAW);
        s.push(crate::print::sel(cur));
        s.push(NL);
        s.push_str("> ");
        s.push_str(&head);
        let stop = self.interaction() == super::interact::ERROR_STOP_MODE;
        let help: &[&str] = if !stop {
            &[]
        } else if self.int_par(crate::val::IntPar::Tracingonline) > 0 {
            &[
                r"This isn't an error message; I'm just \showing something.",
                r"Type `I\show...' to show more (e.g., \show\cs,",
                r"\showthe\count10, \showbox255, \showlists).",
            ]
        } else {
            &[
                r"This isn't an error message; I'm just \showing something.",
                r"Type `I\show...' to show more (e.g., \show\cs,",
                r"\showthe\count10, \showbox255, \showlists).",
                r"And type `I\tracingonline=1\show...' to show boxes and",
                r"lists on your terminal as well as in the transcript file.",
            ]
        };
        self.error_tail(s, cur, super::interact::Help::Lines(help), false, stop);
    }

    /// §311–§318 `show_context`: a backed-up list at the top (`<to be read
    /// again>`, `<recently read>`), `...` for the levels between (IniTeX's
    /// `\errorcontextlines` is 0; macro levels are not named here: CORE.md),
    /// and the reader's own line.
    pub(super) fn show_context(&mut self, s: &mut String) {
        let n = self.st.pending.len();
        let mut middle = n;
        if let Some((list, i, end, true)) = self.st.pending.last().map(|(l, i, e, b)| (l.clone(), i, e, b)) {
            let list = &list[..end];
            middle -= 1;
            let label = if i >= list.len() { "<recently read> " } else { "<to be read again> " };
            let mut read = String::new();
            self.show_toks(&mut read, &list[..i.min(list.len())]);
            let mut rest = String::new();
            self.show_toks(&mut rest, &list[i.min(list.len())..]);
            let _ = write!(s, "{NL}{LINE}T{label}{CUT}{read}{CUT}{rest}{LINE}");
        }
        if middle > 0 {
            let _ = write!(s, "{NL}...");
        }
        let pos = self.st.pos.min(self.text.len());
        let last = pos.saturating_sub(1);
        let start = self.text[..last].iter().rposition(|&c| c == '\r').map_or(0, |k| k + 1);
        let end = self.text[start..].iter().position(|&c| c == '\r').map_or(self.text.len(), |k| start + k);
        let printed = |c: char| -> String {
            let k = u32::from(c);
            if k < 32 || k == 127 {
                let x = if k < 64 { k + 64 } else { k - 64 };
                format!("^^{}", char::from_u32(x).unwrap_or('?'))
            } else {
                c.to_string()
            }
        };
        let before: String = self.text[start..pos.min(end)].iter().map(|&c| printed(c)).collect();
        let after: String =
            if pos < end { self.text[pos..end].iter().map(|&c| printed(c)).collect() } else { String::new() };
        // (the two lines, laid out by the log's gatherer, which knows the
        // line number: `logs::context`)
        let _ = write!(s, "{NL}{LINE}X{before}{CUT}{after}{LINE}");
    }

    /// §218 `show_activities`.
    fn show_activities(&mut self, s: &mut String) {
        s.push(NL);
        s.push(LN);
        let levels = self.open_levels();
        for (p, lv) in levels.iter().enumerate() {
            let _ = write!(s, "{NL}### ");
            s.push_str(match (lv.mode, lv.inner) {
                (Mode::V, false) => "vertical",
                (Mode::V, true) => "internal vertical",
                (Mode::H, false) => "horizontal",
                (Mode::H, true) => "restricted horizontal",
                (Mode::M, false) => "display math",
                (Mode::M, true) => "math",
            });
            let _ = write!(s, " mode entered at line {}", lv.line);
            if lv.mode == Mode::H && !lv.inner {
                let [_, l, lhm, rhm] = lv.hyph_lang;
                if (l, lhm, rhm) != (0, 2, 3) {
                    let _ = write!(s, " (language{l}:hyphenmin{lhm},{rhm})");
                }
            }
            let last = p + 1 == levels.len();
            let cx = self.show_ctx(&lv.items);
            if last {
                // (§986: the current page — the page fold's, not shown —
                // then the contributions)
                if !lv.items.is_empty() {
                    let _ = write!(s, "{NL}### recent contributions:");
                }
            }
            show_list(s, &lv.items, &cx);
            // (§219)
            match lv.mode {
                Mode::V => {
                    let _ = write!(s, "{NL}prevdepth ");
                    if lv.aux <= IGNORE_DEPTH {
                        s.push_str("ignored");
                    } else {
                        crate::print::scaled(s, lv.aux);
                    }
                    if lv.prev_graf != 0 {
                        let _ = write!(s, ", prevgraf {} line", lv.prev_graf);
                        if lv.prev_graf != 1 {
                            s.push('s');
                        }
                    }
                }
                Mode::H => {
                    let _ = write!(s, "{NL}spacefactor {}", lv.aux);
                    if !lv.inner && lv.hyph_lang[0] > 0 {
                        let _ = write!(s, ", current language {}", lv.hyph_lang[0]);
                    }
                }
                Mode::M => {
                    if let Some(n) = &lv.incompleat {
                        s.push_str("this will begin denominator of:");
                        let it = mlist::item(MItem::Noad(n.clone()));
                        let cx = self.show_ctx(&[]);
                        show_list(s, std::slice::from_ref(&it), &cx);
                    }
                }
            }
        }
    }

    /// The open lists, innermost first (tex.web's nest).
    fn open_levels(&mut self) -> Vec<Level> {
        let mut out = Vec::new();
        // (the lists' own state, innermost first: the current one, then
        // each box or math group's record of its enclosing list)
        let mut mode = self.st.mode;
        let mut inner = self.st.inner;
        let mut aux = if mode == Mode::V { self.st.prev_depth } else { self.st.space_factor };
        let mut prev_depth = self.st.prev_depth;
        let mut hyph_lang = self.st.hyph_lang;
        let mut level = self.st.level;
        // (the `k` of each list-making group among the open groups)
        let groups: Vec<(usize, Option<Arc<super::material::BoxGroup>>)> = self
            .st
            .groups
            .0
            .as_ref()
            .map(|v| {
                v.iter()
                    .rev()
                    .enumerate()
                    .filter(|(_, g)| matches!(g.kind, GroupKind::Box | GroupKind::Math))
                    .map(|(k, g)| (k, g.boxg.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let mut gi = 0;
        let mut prev_graf = self.st.prev_graf;
        loop {
            let items = self.level_items(level, mode, inner);
            let paragraph = mode == Mode::H && !inner;
            let line = if paragraph {
                format!("{LINE}Q{level}{LINE}")
            } else if let Some((k, _)) = groups.get(gi) {
                format!("{LINE}G{k}{LINE}")
            } else {
                "0".into()
            };
            let incompleat = if mode == Mode::M { self.incompleat(level) } else { None };
            // (after a paragraph not broken into lines yet: its last line's)
            let shown = if mode == Mode::V && aux == crate::page::MAIN_DEPTH { self.main_depth() } else { aux };
            out.push(Level {
                mode,
                inner,
                line,
                items,
                aux: shown,
                prev_graf: if mode == Mode::V { prev_graf } else { 0 },
                hyph_lang,
                incompleat,
            });
            if paragraph {
                // (the vertical list the paragraph is in, at the same level)
                mode = Mode::V;
                inner = level > 0;
                aux = prev_depth;
                continue;
            }
            let Some((_, Some(g))) = groups.get(gi) else { break };
            gi += 1;
            if level == 0 {
                break;
            }
            level -= 1;
            mode = g.mode;
            inner = g.inner;
            aux = if mode == Mode::V { g.prev_depth } else { g.space_factor };
            prev_depth = g.prev_depth;
            hyph_lang = g.hyph_lang;
            prev_graf = g.prev_graf;
        }
        out
    }

    /// The items of the list at `level` in `mode`, as its gatherer has
    /// them so far (a math list: since its last `\over`).
    fn level_items(&mut self, level: u16, mode: Mode, inner: bool) -> Vec<Item> {
        if self.kind == super::Kind::Other {
            self.kind = super::Kind::Par;
        }
        let nodes = &self.chain.nodes;
        let here = nodes.len();
        let mine = level == self.st.level && mode == self.st.mode;
        match mode {
            Mode::V => {
                let start = if level == 0 {
                    // (the main vertical list: since the page builder was
                    // last called in this piece)
                    let mut from = 0;
                    for (j, n) in nodes.iter().enumerate() {
                        if let Some(p) = n.para()
                            && (p.lb.is_some()
                                || p.before.iter().chain(&p.after).any(|i| matches!(i, Item::BuildPage(_))))
                        {
                            from = j + 1;
                        }
                    }
                    from
                } else {
                    box_start(self.chain, here, level)
                };
                let mut list = Vec::new();
                let mut pd = IGNORE_DEPTH;
                for n in nodes.range(start, nodes.len()) {
                    if n.out.level != level {
                        continue;
                    }
                    let v: Vec<Item> = if level == 0 {
                        n.para().map(|p| p.before.iter().chain(&p.after).cloned().collect()).unwrap_or_default()
                    } else {
                        n.vemit().to_vec()
                    };
                    for it in v {
                        if matches!(it, Item::BuildPage(_)) {
                            list.clear();
                            continue;
                        }
                        push_vertical(&mut list, &mut pd, it);
                    }
                }
                if mine {
                    for it in self.vemit.clone() {
                        push_vertical(&mut list, &mut pd, it);
                    }
                }
                list
            }
            Mode::H | Mode::M => {
                let start = if mode == Mode::H && !inner {
                    super::material::para_start(self.chain, here, level)
                } else {
                    box_start(self.chain, here, level)
                };
                let mut list = Vec::new();
                for n in nodes.range(start, nodes.len()) {
                    if n.out.level == level && n.out.mode == mode {
                        for it in n.emit.iter() {
                            push_horizontal(&mut list, it.clone());
                        }
                    }
                }
                if mine {
                    for it in self.emit.clone() {
                        push_horizontal(&mut list, it);
                    }
                }
                if mode == Mode::M { mlist::current(list) } else { list }
            }
        }
    }

    /// A math list's fraction waiting for its denominator (the list before
    /// its last `\over` as the numerator).
    fn incompleat(&mut self, level: u16) -> Option<mlist::Noad> {
        let nodes = &self.chain.nodes;
        let start = box_start(self.chain, nodes.len(), level);
        let mut list = Vec::new();
        for n in nodes.range(start, nodes.len()) {
            if n.out.level == level && n.out.mode == Mode::M {
                list.extend(n.emit.iter().cloned());
            }
        }
        if level == self.st.level {
            list.extend(self.emit.iter().cloned());
        }
        let at = list.iter().rposition(|i| matches!(i, Item::Noad(m) if matches!(**m, MItem::Above { .. })))?;
        let Item::Noad(m) = &list[at] else { return None };
        let MItem::Above { thickness, left, right } = **m else { return None };
        let (num, _) = mlist::gather(list[..at].iter().cloned(), None);
        let mut n = mlist::Noad::new(mlist::Kind::Fraction { thickness, left, right });
        n.sup = mlist::Field::Mlist(num);
        Some(n)
    }
}
