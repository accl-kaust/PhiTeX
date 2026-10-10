//! Output routines on the graph, the TeX side (CORE.md, "Output routines,
//! insertions and marks"; tex.web 1012–1028): where the page builder fires
//! with a non-empty `\output`, its routine is **a run of nodes** over an
//! empty text whose entry state carries the routine's tokens as pending
//! input ([`output_entry`]); the page fold runs it at the firing place and
//! takes back what its group's `Close` node reports ([`OutputEnd`]).
//!
//! - The run's first node (the internal `\0output`, read before the
//!   routine's tokens) does what tex.web 1017 and 1025 do to the reader:
//!   it defines `\box255` (the page), `\outputpenalty`, `\deadcycles` (as
//!   §1025 increments it) and the marks (§1012–1014; e-TeX's classes too),
//!   all globally, as tex.web makes them there (direct assignments);
//!   then it opens the output group (`GroupKind::Output`) with a new
//!   internal vertical list (`push_nest`, `prev_depth` ignored) and reads
//!   the routine's `{` (`scan_left_brace`).
//! - The second (`\0outputpage`) defines, inside the group, what the
//!   routine reads of the page builder (§421 while `output_active`:
//!   `\pagegoal` … `\pagedepth`; `\insertpenalties`, §1018), so they are
//!   gone when the group ends (§1026: `insert_penalties:=0`; outside a
//!   routine the reads see an empty page); then `normal_paragraph` (§1025).
//! - The group's `}` (§1100, §1026–1028) is the `Close` node: "Unbalanced
//!   output routine" if the routine's text is not read to its end there
//!   (the rest is skipped); the paragraph in progress ends first; then the
//!   group closes, `\box255` must be void ("Output routine didn't use all
//!   of \box255": it is voided), and the routine's vertical list is the
//!   run's result for the fold (the `Close` node's material, after the
//!   pages: [`OutputEnd`]); the pages its
//!   `\shipout`s made are the run's material, in order (as a `\shipout` in
//!   a box's list, they move out of the list).

use std::sync::Arc;

use super::material::box_start;
use super::{
    Chain, Check, Env, Exec, Group, GroupKind, Kind, Mode, Outside, Pending, RState, Run, Scratch, Src, capacity_error,
    outside_read, pending_words, words_of,
};
use crate::hash::FxSet;
use crate::item::{BoxNode, Item};
use crate::page::{IGNORE_DEPTH, Page, push_vertical};
use crate::val::{IntPar, Key, Tok, Val};

/// `Key::Page`: `\pagegoal` (`page_so_far[0]`) … `\pagedepth` (7).
pub const PAGE_GOAL: u8 = 0;
pub const PAGE_DEPTH: u8 = 7;
/// `Key::Page`: `\deadcycles`, `\insertpenalties`.
pub const DEAD_CYCLES: u8 = 8;
pub const INSERT_PENALTIES: u8 = 9;

/// A mark class's `top_mark`, `first_mark`, `bot_mark` (`None`: TeX's
/// `null`, which reads as an empty text).
pub type Marks = [Option<Arc<Vec<Tok>>>; 3];

/// What the page fold reads and computes where it fires (tex.web 1012–1018),
/// for [`output_entry`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputReads {
    /// §1013: the penalty at the best break (`inf_penalty` if there was
    /// none).
    pub output_penalty: i32,
    /// §1018: `insert_penalties` as the fire leaves it (the insertions
    /// held over).
    pub insert_penalties: i32,
    /// `\deadcycles` before the fire (§1025 adds one).
    pub dead_cycles: i32,
    /// Each mark class's marks after this page ([`fire_marks`]), by class.
    pub marks: Vec<(u16, Marks)>,
    /// `page_so_far` as the routine reads it (`\pagegoal` … `\pagedepth`;
    /// §991 has made `\pagedepth` 0).
    pub page_so_far: [i32; 8],
}

/// What an output routine's run starts from, carried by its entry state
/// (the same `Arc` in every node's state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fire {
    /// `\box255` (§1017).
    pub page: Arc<BoxNode>,
    pub output_penalty: i32,
    pub insert_penalties: i32,
    /// After §1025's increment.
    pub dead_cycles: i32,
    pub marks: Vec<(u16, Marks)>,
    pub page_so_far: [i32; 8],
    /// The routine's tokens, its braces included (`\output` as TeX keeps
    /// it, §1226): the list the routine is read from (§1100 checks it is
    /// read to its end).
    pub text: Arc<[Tok]>,
}

/// What the `Close` node of an output routine's group reports to the page
/// fold (§1026–1028). The node's material (`Para::before`, the run's
/// outer level) is the pages the routine's `\shipout`s made
/// (`Item::Shipout`, in order), then the routine's internal vertical list
/// at its end (what it built or `\unvbox`ed, interline glue in it): the
/// fold puts that list before the contributions (after any held-over
/// insertions, §1026), then calls the page builder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputEnd {
    /// How many of the node's items are pages (the rest is the list).
    pub shipped: usize,
    /// `\deadcycles` at the end: after §1025's increment, every
    /// `\shipout` (§638 makes it 0) and assignment in the routine. The
    /// same value is the run's export of `Key::Page(DEAD_CYCLES)`.
    pub dead_cycles: i32,
}

/// The state an output routine's run starts from (tex.web 1025): an empty
/// text, and as input the internal `\0output`, then `output` (`\output`'s
/// tokens, braces included). `page` is the page fired: its box becomes
/// `\box255`. The run is over an empty text: its nodes read only pending
/// tokens, and its `End` node is the input's end.
pub fn output_entry(page: &Page, reads: &OutputReads, output: &[Tok]) -> RState {
    let fire = Fire {
        page: page.b.clone(),
        output_penalty: reads.output_penalty,
        insert_penalties: reads.insert_penalties,
        dead_cycles: reads.dead_cycles + 1,
        marks: reads.marks.clone(),
        page_so_far: reads.page_so_far,
        text: Arc::from(output),
    };
    let mut st = RState::start();
    let begin: Arc<[Tok]> = Arc::from([Tok::Cs(crate::build::CS_OUTPUT)]);
    st.pending = Pending::default();
    st.pending.push((fire.text.clone(), 0, false));
    st.pending.push((begin, 0, false));
    st.fire = Some(Arc::new(fire));
    st
}

/// tex.web 1012–1014 (and etex.web's for each class): the marks after the
/// page `page` fires, from `prev`, those before (by class). The page's
/// marks are those of its list (not inside its boxes), in order.
pub fn fire_marks(prev: &[(u16, Marks)], page: &BoxNode) -> Vec<(u16, Marks)> {
    let mut out = prev.to_vec();
    // (§1012: the bottom mark becomes the top one)
    for (_, m) in &mut out {
        if m[2].is_some() {
            let bot = m[2].clone();
            m[0] = bot;
            m[1] = None;
        }
    }
    // (§1014)
    for it in &page.list {
        let (c, t) = match it {
            Item::Mark(t) => (0, t),
            Item::Marks(c, t) => (*c, t),
            _ => continue,
        };
        let i = match out.binary_search_by_key(&c, |x| x.0) {
            Ok(i) => i,
            Err(i) => {
                out.insert(i, (c, [None, None, None]));
                i
            }
        };
        let m = &mut out[i].1;
        if m[1].is_none() {
            m[1] = Some(t.clone());
        }
        m[2] = Some(t.clone());
    }
    // (§1012: a page without marks has the top one as its first)
    for (_, m) in &mut out {
        if m[0].is_some() && m[1].is_none() {
            let top = m[0].clone();
            m[1] = top;
        }
    }
    out
}

impl OutputEnd {
    /// The routine's list at its end, from its `Close` node `n`.
    pub fn leftover<'a>(&self, n: &'a super::Node) -> &'a [Item] {
        n.para().map_or(&[], |p| p.before.get(self.shipped..).unwrap_or(&[]))
    }
}

/// tex.web 1100's error, then its help.
pub const UNBALANCED: &str = "Unbalanced output routine\n\
    Your sneaky output routine has problematic {'s and/or }'s.\n\
    I can't handle that very well; good luck.";
/// tex.web 1028's error, then its help.
pub const UNUSED: &str = "Output routine didn't use all of \\box255\n\
    Your \\output commands should empty \\box255,\n\
    e.g., by saying `\\shipout\\box255'.\n\
    Proceed; I'll discard its present contents.";

/// What an output routine's run reports (its group's `Close` node), if it
/// ended, and the routine's list at its end.
pub fn output_end(run: &Run) -> Option<(&OutputEnd, &[Item])> {
    run.chain.nodes.iter().find_map(|n| n.output().map(|o| (o, o.leftover(n))))
}

/// An output routine's run, cold: nodes from `entry` until its input ends,
/// as [`super::run_piece`] runs a piece's (`env` answers the reads of
/// names from before the routine's place). The page fold (agent A's
/// sequence) runs it at the firing place; with an entry state in
/// `run_piece`, the same chain (`Chain::with_entry`) is kept and run again
/// as a piece's.
pub fn run_routine(entry: &RState, env: &mut dyn Env) -> Run {
    let text: &[char] = &[];
    let mut outside: Outside = Outside::default();
    let mut chain = Chain::with_entry(entry.clone());
    let mut scratch = Scratch::default();
    let mut state = entry.clone();
    let mut ran = 0u64;
    let mut words = 0u64;
    loop {
        ran += 1;
        let mut x = Exec::new(text, &chain, &state, env, &mut outside, &mut scratch);
        x.command();
        if let Some(Check::Exceeded(size)) = x.stop.take() {
            x.overflow(capacity_error(size));
            x.kind = Kind::End;
        }
        let n = chain.with_group(x.finish());
        let end = n.kind == Kind::End;
        words += words_of(&n);
        let verdict = env.check(ran, words.max(pending_words(&n.out)));
        state = n.out.clone();
        chain.push_arc(n);
        if end {
            break;
        }
        // (a routine that never ends its input: TeX's memory runs out)
        if let Check::Exceeded(size) = verdict {
            let mut x = Exec::new(text, &chain, &state, env, &mut outside, &mut scratch);
            x.overflow(capacity_error(size));
            x.kind = Kind::End;
            let n = chain.with_group(x.finish());
            chain.push_arc(n);
            break;
        }
    }
    chain.nodes.freeze();
    let end = chain.nodes.len();
    let mut exports: Vec<(Key, Val)> =
        chain.defs_of.keys().filter_map(|k| chain.resolve(k, end).map(|(_, v)| (k.clone(), v))).collect();
    exports.sort_by(|a, b| a.0.cmp(&b.0));
    let paras = chain.para_at.iter().filter_map(|&i| chain.nodes[i as usize].para().cloned()).collect();
    let errors = chain.err_at.iter().flat_map(|&i| chain.nodes[i as usize].errors().iter().cloned()).collect();
    // (each read from before the routine: its first reader, one per name
    // and value)
    let mut reads: Vec<(Key, u32, Val)> = Vec::new();
    let mut seen: FxSet<usize> = FxSet::default();
    for (i, n) in chain.nodes.iter().enumerate() {
        for r in n.reads.iter() {
            if matches!(r.src, Src::Node(_)) || !seen.insert(Arc::as_ptr(r) as usize) {
                continue;
            }
            if !reads.iter().any(|x| x.0 == r.key && x.2 == r.val) {
                reads.push((r.key.clone(), i as u32, r.val.clone()));
            }
        }
    }
    reads.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut fonts: Vec<(u32, Arc<str>)> = chain
        .defs_of
        .iter()
        .filter_map(|(k, v)| match k {
            Key::FontLoaded(f) => v.first().map(|&(i, _)| (i, f.clone())),
            _ => None,
        })
        .collect();
    fonts.sort_by_key(|x| x.0);
    Run {
        chain,
        reads,
        exports,
        paras,
        errors,
        ran,
        fonts: fonts.into_iter().map(|(_, f)| f).collect(),
        abandoned: false,
        lean: None,
        crossed: false,
    }
}

impl Exec<'_> {
    /// `\0output`: tex.web 1017's definitions and 1025 (the page builder's
    /// part done: the page is the entry's).
    pub(super) fn output_begin(&mut self) {
        let Some(f) = self.st.fire.clone() else { return };
        self.define(Key::Box(255), Val::Box(f.page.clone()), true);
        self.define(Key::IntPar(IntPar::Outputpenalty), Val::Int(f.output_penalty), true);
        self.define(Key::Page(DEAD_CYCLES), Val::Int(f.dead_cycles), true);
        for (c, m) in &f.marks {
            for (w, t) in m.iter().enumerate() {
                self.define(Key::Mark(w as u8, *c), Val::Toks(t.clone().unwrap_or_default()), true);
            }
        }
        // (§1025: `push_nest; mode:=-vmode; prev_depth:=ignore_depth`, the
        // group, its `{`)
        let at = self.chain.nodes.len() as u32;
        self.st.groups.push(Group { kind: GroupKind::Output, after: Vec::new(), boxg: None, at, align: None });
        self.kind = Kind::Open;
        self.st.level += 1;
        self.st.mode = Mode::V;
        self.st.inner = true;
        self.st.prev_depth = IGNORE_DEPTH;
        self.st.prev_graf = 0;
        self.scan_left_brace();
        let t = self.cs("\u{0}outputpage");
        self.rewrite(vec![t]);
    }

    /// `\0outputpage`: inside the output group, the page builder's state as
    /// the routine reads it, then `normal_paragraph` (§1025).
    pub(super) fn output_page_state(&mut self) {
        let Some(f) = self.st.fire.clone() else { return };
        for (k, v) in f.page_so_far.iter().enumerate() {
            self.define(Key::Page(PAGE_GOAL + k as u8), Val::Int(*v), false);
        }
        self.define(Key::Page(INSERT_PENALTIES), Val::Int(f.insert_penalties), false);
        self.normal_paragraph();
    }

    /// §1100: the routine's text must be read to its end at its group's
    /// `}` (`loc=null`, the current list the routine's or a backed-up
    /// one); else "Unbalanced output routine" and the rest of the list is
    /// skipped (`repeat get_token until loc=null`). Then the list ends
    /// (`end_token_list`).
    fn output_text_end(&mut self) {
        let text = self.st.fire.as_ref().map(|f| f.text.clone());
        let Some((list, _, _, backed)) = self.st.pending.last().map(|(l, i, e, b)| (l.clone(), i, e, b)) else {
            return;
        };
        let done = self.st.pending.top_done() == Some(true);
        let ours = text.is_some_and(|x| Arc::ptr_eq(&x, &list));
        if !done || !(ours || backed) {
            self.err(UNBALANCED.into());
            if done {
                // (a list read to its end: `get_token` reads on in the one
                // below it, to its end)
                self.st.pending.pop();
            }
            self.st.pending.skip_top();
        }
        self.st.pending.pop();
    }

    /// The `}` of the output group (tex.web 1100, 1026–1028): a `Close`
    /// node reporting the routine's end ([`OutputEnd`]).
    pub(super) fn output_close(&mut self, t: &Tok) {
        self.output_text_end();
        if self.st.mode == Mode::H && !self.st.inner {
            // (`end_graf` before the group ends: the paragraph ends, then the
            // `}` again, backed up: read to its end, §1100 lets it pass)
            let e = self.cs("\u{0}endgraf");
            self.back(t.clone());
            self.rewrite(vec![e]);
            return;
        }
        let after = self.st.groups.pop().map(|g| g.after).unwrap_or_default();
        // (the routine's list, from the vertical items its nodes emitted)
        let inner = self.st.level;
        let nodes = &self.chain.nodes;
        let end = nodes.len();
        let start = box_start(self.chain, end, inner);
        let mut list = Vec::new();
        let mut pd = IGNORE_DEPTH;
        for n in nodes.range(start, end) {
            if n.out.level == inner {
                for it in n.vemit() {
                    push_vertical(&mut list, &mut pd, it.clone());
                }
            }
        }
        for it in std::mem::take(&mut self.vemit) {
            push_vertical(&mut list, &mut pd, it);
        }
        let ships: Vec<Item> = list.iter().filter(|i| matches!(i, Item::Shipout(_))).cloned().collect();
        list.retain(|i| !matches!(i, Item::Shipout(_) | Item::BuildPage(_) | Item::End(_)));
        // (`pop_nest`: the run's outer list, the main vertical list's place)
        self.st.level -= 1;
        self.st.mode = Mode::V;
        self.st.inner = self.st.level > 0;
        // (the main vertical list's, as it was where the routine fired)
        self.st.prev_depth = crate::page::MAIN_DEPTH;
        self.st.prev_graf = 0;
        // (§1028, once the group has ended)
        if matches!(self.after_close(&Key::Box(255)), Val::Box(_)) {
            self.err(UNUSED.into());
            self.define(Key::Box(255), Val::Void, true);
        }
        let dead_cycles = match self.after_close(&Key::Page(DEAD_CYCLES)) {
            Val::Int(i) => i,
            _ => 0,
        };
        let shipped = ships.len();
        self.vemit.extend(ships);
        self.vemit.extend(list);
        self.output = Some(Box::new(OutputEnd { shipped, dead_cycles }));
        self.rewrite(after);
        self.closes();
    }

    /// The definition of `key` reaching the node after this one, which
    /// closes the innermost group: the one reaching this node, unless it
    /// is local to that group (then the one before it).
    fn after_close(&mut self, key: &Key) -> Val {
        let inside = self.get(key);
        let chain = self.chain;
        let here = chain.nodes.len();
        let og = chain.open_groups().last().copied();
        let mut local = false;
        if let Some(ds) = chain.defs_of.get(key) {
            for &(d, j) in ds.iter().rev() {
                let n = &chain.nodes[d as usize];
                let def = &n.defs()[j as usize];
                if def.global {
                    return def.val.clone();
                }
                let g = chain.def_group(n, def);
                let closed = g.is_some_and(|g| chain.close_of.get(&g).is_some_and(|&c| (c as usize) < here));
                if closed {
                    continue;
                }
                if og.is_some() && g == og {
                    local = true;
                    continue;
                }
                return def.val.clone();
            }
        }
        if local { outside_read(self.env, self.outside, key, here as u32).val.clone() } else { inside }
    }

    /// `\deadcycles=`, `\insertpenalties=` (tex.web 1246), `\pagegoal=` …
    /// (1245): the page builder's quantity, assigned directly.
    pub(super) fn alter_page(&mut self, p: crate::val::Prim) {
        let (key, v) = match p {
            crate::val::Prim::PageInt(k) => {
                self.optional_equals();
                (Key::Page(DEAD_CYCLES + k), self.scan_int())
            }
            crate::val::Prim::PageDimen(k) => {
                self.optional_equals();
                (Key::Page(PAGE_GOAL + k), self.scan_dimen())
            }
            _ => return,
        };
        // (and the page builder's quantity changes where the main vertical
        // list is now: an item in its order, tex.web 1245)
        if let Key::Page(i) = key {
            if self.st.level == 0 {
                self.vemit.push(Item::PageSoFar(i, v));
            } else if self.st.fire.is_none() {
                // (inside a group, a box, a paragraph or a formula, not an
                // output routine: TeX assigns `page_so_far` at once all the
                // same, so the item goes to the fold's stream at this node,
                // as `Item::Halt` does; no page builder call comes between)
                let p = self.para.get_or_insert_with(|| Arc::new(super::Para::default()));
                Arc::make_mut(p).before.push(Item::PageSoFar(i, v));
            }
        }
        self.change_direct(key, Val::Int(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(s: &str) -> Arc<Vec<Tok>> {
        Arc::new(s.chars().map(|c| Tok::Char(11, c)).collect())
    }

    #[test]
    fn marks_as_fire_up_makes_them() {
        let page = |marks: &[&str]| BoxNode {
            list: marks.iter().map(|m| Item::Mark(mark(m))).collect(),
            ..BoxNode::default()
        };
        // (the first page: first and bottom marks; no top mark)
        let m1 = fire_marks(&[], &page(&["a", "b", "c"]));
        assert_eq!(m1, vec![(0, [None, Some(mark("a")), Some(mark("c"))])]);
        // (a page without marks: top, first and bottom all the last bottom)
        let m2 = fire_marks(&m1, &page(&[]));
        assert_eq!(m2, vec![(0, [Some(mark("c")), Some(mark("c")), Some(mark("c"))])]);
        // (a page with one)
        let m3 = fire_marks(&m2, &page(&["d"]));
        assert_eq!(m3, vec![(0, [Some(mark("c")), Some(mark("d")), Some(mark("d"))])]);
        // (e-TeX's classes apart)
        let p = BoxNode { list: vec![Item::Marks(3, mark("x")), Item::Mark(mark("e"))], ..BoxNode::default() };
        let m4 = fire_marks(&m3, &p);
        assert_eq!(
            m4,
            vec![
                (0, [Some(mark("d")), Some(mark("e")), Some(mark("e"))]),
                (3, [None, Some(mark("x")), Some(mark("x"))])
            ]
        );
    }
}
