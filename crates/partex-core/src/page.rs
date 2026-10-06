//! Parts 44–45: Breaking vertical lists into pages (§967–§979) and The
//! page builder (§980–§1028), with `normal_paragraph` (§1070).
//!
//! The engine's [`Builder`] is the current page; this module feeds it
//! the contribution list one node at a time (each a recorded call, a
//! step), prints what it reports, and runs the output routine (or ships
//! the page out), a recorded call too.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::mem;

use partex_engine::builder::{
    self, After, Builder, Contents, Event, InsBox, InsState, MarkClasses, Marks, Step,
};
use partex_engine::node::{BoxNode, GlueSpec, Node, Tokens};
use partex_engine::pack::Spec;
use partex_engine::page::{self as vpage, SplitParams};

use crate::arith::Scaled;
use crate::host::Host;
use crate::mem::NULL;
use crate::nest::IGNORE_DEPTH;
use crate::ssa::Func;
use crate::tex::{Jump, Tex};
use crate::track::{Row, Tracker, page as page_row, scalar};
use crate::web::*;
use partex_ssa::Version;

const TOP_MARK: usize = 0;
const FIRST_MARK: usize = 1;
const BOT_MARK: usize = 2;
const SPLIT_FIRST_MARK: usize = 3;
const SPLIT_BOT_MARK: usize = 4;
/// The same indices as mark codes (`Tex::mark_row`).
const TOP_MARK_I: i32 = 0;
const FIRST_MARK_I: i32 = 1;
const BOT_MARK_I: i32 = 2;
const SPLIT_FIRST_MARK_I: i32 = 3;
const SPLIT_BOT_MARK_I: i32 = 4;
/// §833, §974
const AWFUL_BAD: i32 = 0o7777777777;

/// The versions of the page's slots (`track::page`).
type PageFields = [u128; page_row::COUNT as usize];

/// What a step of `build_page` did ([`Tex::page_step`]'s result).
#[derive(Hash)]
struct Stepped {
    /// The node that goes on the page (§998), or one discarded to keep
    /// for `\pagediscards` (§999): `build_page` links it in.
    page: Option<Node>,
    discard: Option<Node>,
    /// The nodes that go back in front of the contributions: the node
    /// behind `\topskip` glue (§1001), a kern that ends them (§1000), or
    /// the rest of a fired page and the break node (§1017).
    front: Vec<Node>,
    then: Then,
}

/// What `build_page` does after a step.
#[derive(Hash)]
enum Then {
    /// Take the next contribution.
    Next,
    /// Return (§1000's kern).
    Return,
    /// The page fired (§1005): run the output routine, over box 255 and
    /// the insertion boxes of these classes.
    Fired(Vec<u8>),
    /// The page is complete and the fire waits for the next
    /// `big_switch` (DESIGN §7.16.1, "The deferred fire's form"): return.
    Defer,
}

/// The fields of the page so far that the step of node `p` reads on
/// every path from here, as a mask of `track::page` slots: the page's part
/// of the step's name (DESIGN 7.17.2). They are some of the step's reads,
/// which the engine reports as TeX's step makes them (`builder::Access`).
/// The path is known from `p`, what follows a kern, `page_contents` and
/// whether the page's last node precedes a break. A fire reads the whole
/// page, including the fields a path that does not fire would have read.
fn step_name(p: &Node, after: After, page: &Builder) -> u32 {
    use crate::track::page::*;
    const fn mask(fields: &[u8]) -> u32 {
        let mut m = 0;
        let mut i = 0;
        while i < fields.len() {
            m |= 1 << fields[i];
            i += 1;
        }
        m
    }
    const GOAL: u8 = SO_FAR;
    const TOTAL: u8 = SO_FAR + 1;
    const SHRINK: u8 = SO_FAR + 6;
    const DEPTH: u8 = SO_FAR + 7;
    // (§1003: the depth and its limit)
    const CONTRIBUTE: u32 = mask(&[DEPTH, MAX_DEPTH]);
    // (§1002)
    const BOX: u32 = mask(&[CONTENTS, TOTAL]) | CONTRIBUTE;
    // (§1005, §1007: the badness's first test, the insertion penalties and
    // the cost to beat)
    const BREAK: u32 = mask(&[GOAL, TOTAL, INSERT_PENALTIES, LEAST_COST]);
    // (§1004 for a kern, then §1003)
    const HEIGHTS: u32 = mask(&[TOTAL]) | CONTRIBUTE;
    let contents = mask(&[CONTENTS]);
    let started = page.contents == Contents::BoxThere;
    match p {
        Node::Box(_) | Node::Rule { .. } if started => BOX,
        Node::Box(_) | Node::Rule { .. } => contents,
        Node::Whatsit(_) | Node::Mark(_) => CONTRIBUTE,
        Node::Glue { .. } | Node::Leaders(_) | Node::Kern { .. } | Node::Penalty(_) if !started => {
            contents
        }
        Node::Glue { .. } | Node::Leaders(_) => {
            // (§1004 for glue: its stretch's order and the shrink)
            let spec = match p {
                Node::Leaders(l) => l.spec,
                Node::Glue { spec, .. } => *spec,
                _ => unreachable!(),
            };
            let order = spec.stretch_order as u8;
            let glue = mask(&[SHRINK]) | 1 << (SO_FAR + 2 + order);
            let breaks = if page.tail_precedes_break() { BREAK } else { 0 };
            contents | mask(&[LIST_TAIL]) | breaks | glue | HEIGHTS
        }
        Node::Kern { .. } => match after {
            After::Nothing => contents,
            After::Glue => contents | BREAK | HEIGHTS,
            After::Other => contents | HEIGHTS,
        },
        Node::Penalty(pi) if *pi < 10000 => contents | BREAK | CONTRIBUTE,
        Node::Penalty(_) => contents | CONTRIBUTE,
        // (§1008: without a page, the freeze assigns the depth and its
        // limit before §1003 reads them)
        Node::Ins(_) if page.contents == Contents::Empty => mask(&[CONTENTS, INS]),
        Node::Ins(_) => mask(&[CONTENTS, INS]) | CONTRIBUTE,
        _ => 0,
    }
}

/// The builder's parameters and insertion registers, read (and recorded)
/// where the builder reads them.
impl<H: Host, T: Tracker> builder::Env for Tex<H, T> {
    fn vsize(&self) -> Scaled {
        self.dimen_par(VSIZE_CODE)
    }
    fn max_depth(&self) -> Scaled {
        self.dimen_par(MAX_DEPTH_CODE)
    }
    fn top_skip(&self) -> GlueSpec {
        self.glue_par(TOP_SKIP_CODE)
    }
    fn tracing(&self) -> bool {
        self.int_par(TRACING_PAGES_CODE) > 0
    }
    fn save_discards(&self) -> bool {
        self.int_par(SAVING_VDISCARDS_CODE) > 0
    }
    fn ins_box(&self, n: u8) -> InsBox {
        match self.box_reg(i32::from(n)) {
            None => InsBox::Void,
            Some(b) if !b.vertical => InsBox::Hbox,
            Some(b) => InsBox::Vbox {
                height: b.height,
                depth: b.depth,
            },
        }
    }
    fn count(&self, n: u8) -> i32 {
        Tex::count(self, i32::from(n))
    }
    fn dimen(&self, n: u8) -> Scaled {
        Tex::dimen(self, i32::from(n))
    }
    fn skip(&self, n: u8) -> GlueSpec {
        Tex::skip(self, i32::from(n))
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §1070
    pub(crate) fn normal_paragraph(&mut self) -> Result<(), Jump> {
        if self.int_par(LOOSENESS_CODE) != 0 {
            self.eq_word_define(INT_BASE + LOOSENESS_CODE, 0)?;
        }
        if self.dimen_par(HANG_INDENT_CODE) != 0 {
            self.eq_word_define(DIMEN_BASE + HANG_INDENT_CODE, 0)?;
        }
        if self.int_par(HANG_AFTER_CODE) != 1 {
            self.eq_word_define(INT_BASE + HANG_AFTER_CODE, 1)?;
        }
        if self.par_shape().is_some() {
            self.eq_define(PAR_SHAPE_LOC, SHAPE_REF, NULL)?;
        }
        if self.penalties(INTER_LINE_PENALTIES_LOC).is_some() {
            self.eq_define(INTER_LINE_PENALTIES_LOC, SHAPE_REF, NULL)?;
        }
        Ok(())
    }

    /// The current mark `t` (`top_mark_code` ..) of mark class `class`.
    pub(crate) fn mark(&self, class: i32, t: i32) -> Option<Tokens> {
        self.mark_read(class, t);
        self.cur_mark.get(&class)?[usize::try_from(t).ok()?].clone()
    }

    /// The row of mark `t` of class `class` (DESIGN 7.17.12, `cur_mark`:
    /// marks by class, each a shared token list carrying its version).
    fn mark_row(class: i32, t: i32) -> crate::track::Row {
        crate::track::Row::Mark(
            class
                .cast_unsigned()
                .wrapping_mul(5)
                .wrapping_add(t.cast_unsigned()),
        )
    }

    #[inline]
    fn mark_read(&self, class: i32, t: i32) {
        if T::VALUES {
            self.tracker
                .value_read(Self::mark_row(class, t), || self.mark_version(class, t));
        }
    }

    #[inline]
    fn mark_wrote(&self, class: i32, t: i32) {
        if T::VALUES {
            self.tracker.value_wrote(Self::mark_row(class, t));
        }
    }

    /// Mark `t` of class `class`'s version: its list's, made when the list
    /// was made (none: absent).
    pub(crate) fn mark_version(&self, class: i32, t: i32) -> u128 {
        self.cur_mark
            .get(&class)
            .and_then(|m| m.get(usize::try_from(t).ok()?)?.as_ref())
            .map_or(partex_ssa::Version::ABSENT.0, |l| l.version())
    }

    /// §977: extract a page of height `h` from box `n`.
    pub(crate) fn vsplit(&mut self, n: i32, h: Scaled) -> Result<Option<Arc<BoxNode>>, Jump> {
        self.split_discards_mut().clear();
        let classes: alloc::vec::Vec<i32> = self.cur_mark.keys().copied().collect();
        for &c in &classes {
            if let Some(m) = self.cur_mark.get_mut(&c) {
                m[SPLIT_FIRST_MARK] = None;
                m[SPLIT_BOT_MARK] = None;
            }
            self.mark_wrote(c, SPLIT_FIRST_MARK_I);
            self.mark_wrote(c, SPLIT_BOT_MARK_I);
        }
        // §978: dispense with trivial cases of void or bad boxes.
        let Some(v) = self.box_reg(n) else {
            return Ok(None);
        };
        if !v.vertical {
            self.print_err(b"");
            self.print_esc(b"vsplit");
            self.print_str(b" needs a ");
            self.print_esc(b"vbox");
            self.help(&[
                b"The box you are trying to split is an \\hbox.",
                b"I can't split such a box, so I'll leave it alone.",
            ]);
            self.error()?;
            return Ok(None);
        }
        let params = SplitParams {
            split_max_depth: self.dimen_par(SPLIT_MAX_DEPTH_CODE),
            split_top_skip: self.glue_par(SPLIT_TOP_SKIP_CODE),
            save_discards: self.int_par(SAVING_VDISCARDS_CODE) > 0,
        };
        let split = match vpage::vsplit(v, h, &params) {
            Ok(s) => s,
            Err(c) => return self.confusion(c.0.as_bytes()),
        };
        for _ in 0..split.infinite_shrink {
            self.infinite_shrink_split_error()?;
        }
        // §979
        *self.split_discards_mut() = split.discards.into();
        for (class, (first, bot)) in split.marks {
            let m = self.cur_mark.entry(class).or_default();
            m[SPLIT_FIRST_MARK] = Some(first);
            m[SPLIT_BOT_MARK] = Some(bot);
            self.mark_wrote(class, SPLIT_FIRST_MARK_I);
            self.mark_wrote(class, SPLIT_BOT_MARK_I);
        }
        // §977: the rest into box `n`, then the split packed to `h`
        let rest = match split.rest {
            Some(rest) => Some(self.vpack(rest, Spec::NATURAL)?.share()),
            None => None,
        };
        self.set_box_reg(n, rest);
        let b = self.vpack_call(split.split, Spec::Exactly(h), params.split_max_depth)?;
        Ok(Some(b.node.share()))
    }

    /// §976: the error for infinitely shrinkable glue in a split box
    /// (in pdfTeX and `XeTeX`, with bit 1 of `\ignoreprimitiveerror`, a
    /// line of the log: `print_ignored_err`, its prefix not counted in
    /// `file_offset`).
    fn infinite_shrink_split_error(&mut self) -> Result<(), Jump> {
        if self.params.flavor != crate::params::Flavor::Tex
            && self.int_par(IGNORE_PRIMITIVE_ERROR_CODE) % 2 != 0
        {
            let old_setting = self.selector();
            self.set_selector(LOG_ONLY);
            self.wlog_bytes(b"\nignored: ");
            self.print_str(b"Infinite glue shrinkage found in box being split");
            self.set_selector(old_setting);
            return Ok(());
        }
        self.print_err(b"Infinite glue shrinkage found in box being split");
        self.help(&[
            b"The box you are \\vsplitting contains some infinitely",
            b"shrinkable glue, e.g., `\\vss' or `\\vskip 0pt minus 1fil'.",
            b"Such glue doesn't belong there; but you can safely proceed,",
            b"since the offensive shrinkability has been made finite.",
        ]);
        self.error()
    }

    /// §985
    pub(crate) fn print_totals(&mut self, so_far: &[Scaled; 8]) {
        self.print_scaled(so_far[1]);
        for (k, unit) in [(2, &b""[..]), (3, b"fil"), (4, b"fill"), (5, b"filll")] {
            if so_far[k] != 0 {
                self.print_str(b" plus ");
                self.print_scaled(so_far[k]);
                self.print_str(unit);
            }
        }
        if so_far[6] != 0 {
            self.print_str(b" minus ");
            self.print_scaled(so_far[6]);
        }
    }

    // The page builder's state by field (DESIGN 7.17.12, the `page` row,
    // `track::page`): each read tells the tracker the field's version (and
    // a machine its `MCell::Page`), each change is a read of it and a
    // write.
    #[inline]
    fn page_read(&self, f: u8) {
        self.tracker.page_access(false);
        if T::VALUES {
            self.tracker
                .value_read(Row::Page(f), || self.page_field_version(f));
        }
    }

    #[inline]
    fn page_changed(&self, f: u8) {
        self.tracker.page_access(true);
        if T::VALUES {
            self.tracker
                .value_read(Row::Page(f), || self.page_field_version(f));
            self.tracker.value_wrote(Row::Page(f));
            if f == page_row::LIST {
                // (and what a step reads of it)
                self.tracker.value_wrote(Row::Page(page_row::LIST_LEN));
                self.tracker.value_wrote(Row::Page(page_row::LIST_TAIL));
            }
        }
    }

    /// The version of the `page` row's field `f` now, made from the value
    /// (the lists carry theirs; the insertion records, a few per page, by
    /// their contents).
    pub(crate) fn page_field_version(&self, f: u8) -> u128 {
        use crate::track::page::*;
        let p = &self.page;
        let int = crate::track::scalar_version_i32;
        match f {
            CONTENTS => int(p.contents as i32),
            LIST => p.list.version().0,
            MAX_DEPTH => int(p.max_depth),
            LEAST_COST => int(p.least_cost),
            BEST_BREAK => Version::of(&p.best_break).0,
            BEST_SIZE => int(p.best_size),
            INS => Version::of(&p.ins).0,
            INSERT_PENALTIES => int(p.insert_penalties),
            LAST_GLUE => Version::of(&p.last.glue).0,
            LAST_PENALTY => int(p.last.penalty),
            LAST_KERN => int(p.last.kern),
            LAST_NODE_TYPE => int(p.last.node_type),
            DISCARDS => p.discards.version().0,
            SPLIT_DISCARDS => self.split_discards.version().0,
            LIST_LEN => int(i32::try_from(p.list.len()).unwrap_or(i32::MAX)),
            LIST_TAIL => int(i32::from(p.tail_precedes_break())),
            k => int(p.so_far[usize::from(k - SO_FAR) & 7]),
        }
    }

    /// The versions of the page's slots now.
    fn page_fields(&self) -> PageFields {
        let mut v = [0; page_row::COUNT as usize];
        for (f, x) in (0..page_row::COUNT).zip(v.iter_mut()) {
            *x = self.page_field_version(f);
        }
        v
    }

    /// The builder out, for a step (the engine's step runs on it and
    /// reports what it read and assigned, `builder::Access`).
    fn page_take(&mut self) -> Builder {
        self.tracker.page_access(true);
        mem::take(&mut self.page)
    }

    /// The builder back after a step: its reads are the fields the step
    /// read, at their versions `before` it ran, and its writes every
    /// field it assigned, whether or not the value changed (DESIGN
    /// 7.17.2: a write of an equal value is kept in the record).
    fn page_put(&mut self, page: Builder, acc: builder::Access, before: Option<&PageFields>) {
        self.page = page;
        if let Some(v) = before {
            for f in acc.reads() {
                self.tracker.value_read(Row::Page(f), || v[usize::from(f)]);
            }
            for f in acc.assigned() {
                self.tracker.value_wrote(Row::Page(f));
            }
        }
    }

    /// The builder whole, for the engine's routines that change it
    /// (§1012, §1023): every field read and written, the list read whole
    /// (its writes are noted once the routine is done,
    /// [`Self::page_list_wrote`]).
    fn page_all_mut(&mut self) -> &mut Builder {
        for f in 0..page_row::BUILDER {
            if f != page_row::LIST {
                self.page_changed(f);
            }
        }
        self.page_list();
        &mut self.page
    }

    /// `page_contents` (§980).
    pub(crate) fn page_contents(&self) -> Contents {
        self.page_read(page_row::CONTENTS);
        self.page.contents
    }
    /// The page so far (§980), read whole: its length and each node, the
    /// appends since it began (DESIGN 7.17.3 item 5).
    pub(crate) fn page_list(&self) -> &partex_engine::nodelist::NodeList {
        self.page_read(page_row::LIST_LEN);
        if T::VALUES {
            for (k, n) in self.page.list.iter().enumerate() {
                let k = u32::try_from(k).unwrap_or(u32::MAX);
                self.tracker
                    .value_read(Row::PageNode(k), || Version::of(n).0);
            }
        }
        &self.page.list
    }
    /// The page's length (`LIST_LEN`), not its nodes.
    pub(crate) fn page_list_len(&self) -> usize {
        self.page_read(page_row::LIST_LEN);
        self.page.list.len()
    }
    /// §998: `p` goes on the page, the page's next node: a read of the
    /// length and an append, never a read of the list.
    pub(crate) fn page_push(&mut self, p: Node) {
        self.page_read(page_row::LIST_LEN);
        self.tracker.page_access(true);
        let k = u32::try_from(self.page.list.len()).unwrap_or(u32::MAX);
        self.page.list.push(p);
        if T::VALUES {
            self.tracker.value_wrote(Row::PageNode(k));
            self.tracker.value_wrote(Row::Page(page_row::LIST_LEN));
            self.tracker.value_wrote(Row::Page(page_row::LIST_TAIL));
        }
    }
    /// The page so far, taken whole (§1023, §1026): it is empty after.
    pub(crate) fn page_take_list(&mut self) -> partex_engine::nodelist::NodeList {
        self.page_list();
        self.tracker.page_access(true);
        let list = mem::take(&mut self.page.list);
        self.page_list_wrote();
        list
    }
    /// The page's list was changed whole: its length, its tail and each
    /// node it holds are written.
    fn page_list_wrote(&self) {
        if T::VALUES {
            self.tracker.value_wrote(Row::Page(page_row::LIST_LEN));
            self.tracker.value_wrote(Row::Page(page_row::LIST_TAIL));
            for k in 0..self.page.list.len() {
                let k = u32::try_from(k).unwrap_or(u32::MAX);
                self.tracker.value_wrote(Row::PageNode(k));
            }
        }
    }
    /// The version of the page's `k`-th node now (`Row::PageNode`).
    pub(crate) fn page_node_version(&self, k: usize) -> u128 {
        self.page
            .list
            .get(k)
            .map_or(Version::ABSENT.0, |n| Version::of(n).0)
    }
    /// `page_so_far[k]` (§982).
    pub(crate) fn page_so_far(&self, k: usize) -> Scaled {
        self.page_read(page_row::SO_FAR + u8::try_from(k & 7).unwrap_or(0));
        self.page.so_far[k & 7]
    }
    pub(crate) fn set_page_so_far(&mut self, k: usize, v: Scaled) {
        self.page_changed(page_row::SO_FAR + u8::try_from(k & 7).unwrap_or(0));
        self.page.so_far[k & 7] = v;
    }
    /// The page insertion records (§981).
    pub(crate) fn page_ins(&self) -> &[builder::PageIns] {
        self.page_read(page_row::INS);
        &self.page.ins
    }
    /// `insert_penalties` (§982).
    pub(crate) fn insert_penalties(&self) -> i32 {
        self.page_read(page_row::INSERT_PENALTIES);
        self.page.insert_penalties
    }
    pub(crate) fn set_insert_penalties(&mut self, v: i32) {
        self.page_changed(page_row::INSERT_PENALTIES);
        self.page.insert_penalties = v;
    }
    /// `last_glue`, `last_penalty`, `last_kern` and `last_node_type`
    /// (§982, e-TeX), each its own field.
    pub(crate) fn page_last_glue(&self) -> Option<GlueSpec> {
        self.page_read(page_row::LAST_GLUE);
        self.page.last.glue
    }
    pub(crate) fn page_last_penalty(&self) -> i32 {
        self.page_read(page_row::LAST_PENALTY);
        self.page.last.penalty
    }
    pub(crate) fn page_last_kern(&self) -> Scaled {
        self.page_read(page_row::LAST_KERN);
        self.page.last.kern
    }
    pub(crate) fn page_last_node_type(&self) -> i32 {
        self.page_read(page_row::LAST_NODE_TYPE);
        self.page.last.node_type
    }
    /// e-TeX's `page_disc`, to change.
    pub(crate) fn page_discards_mut(&mut self) -> &mut partex_engine::nodelist::NodeList {
        self.page_changed(page_row::DISCARDS);
        &mut self.page.discards
    }
    /// e-TeX's `split_disc` (§977), to change.
    pub(crate) fn split_discards_mut(&mut self) -> &mut partex_engine::nodelist::NodeList {
        if T::VALUES {
            let f = page_row::SPLIT_DISCARDS;
            self.tracker
                .value_read(Row::Page(f), || self.page_field_version(f));
            self.tracker.value_wrote(Row::Page(f));
        }
        &mut self.split_discards
    }

    /// §986: show the status of the current page.
    pub(crate) fn show_page_status(&mut self) {
        if self.page_list_len() == 0 {
            return;
        }
        self.print_nl(b"### current page:");
        if self.output_active() {
            self.print_str(b" (held over for next output)");
        }
        // (shown from a copy: showing does not change the page)
        let list = self.page_list().to_vec();
        self.show_box(&list);
        if self.page_contents() > Contents::Empty {
            self.print_nl(b"total height ");
            let so_far: [Scaled; 8] = core::array::from_fn(|k| self.page_so_far(k));
            self.print_totals(&so_far);
            self.print_nl(b" goal height ");
            self.print_scaled(so_far[0]);
            for r in self.page_ins().to_vec() {
                self.print_ln();
                self.print_esc(b"insert");
                let n = i32::from(r.number);
                self.print_int(n);
                self.print_str(b" adds ");
                let t = if self.count(n) == 1000 {
                    r.height
                } else {
                    let c = self.count(n);
                    self.x_over_n(r.height, 1000) * c
                };
                self.print_scaled(t);
                if let InsState::SplitUp { broken_ins, .. } = r.state {
                    let t = self
                        .page_list()
                        .iter()
                        .take(broken_ins + 1)
                        .filter(|p| matches!(p, Node::Ins(i) if i.number == r.number))
                        .count();
                    self.print_str(b", #");
                    self.print_int(i32::try_from(t).unwrap_or(i32::MAX));
                    self.print_str(b" might split");
                }
            }
        }
    }

    /// §992: delete box `n` after an error.
    pub(crate) fn box_error(&mut self, n: i32) -> Result<(), Jump> {
        self.error()?;
        self.begin_diagnostic();
        self.print_nl(b"The following box has been deleted:");
        if let Some(b) = self.box_reg(n).cloned() {
            self.show_box_node(&b);
        }
        self.end_diagnostic(true);
        self.set_box_reg(n, None);
        Ok(())
    }

    /// §993
    pub(crate) fn ensure_vbox(&mut self, n: i32) -> Result<(), Jump> {
        if self.box_reg(n).is_some_and(|b| !b.vertical) {
            self.print_err(b"Insertions can only be added to a vbox");
            self.help(&[
                b"Tut tut: You're trying to \\insert into a",
                b"\\box register that now contains an \\hbox.",
                b"Proceed, and I'll discard its present contents.",
            ]);
            self.box_error(n)?;
        }
        Ok(())
    }

    /// §994: append contributions to the current page, one step per
    /// node ([`Self::page_step`]).
    pub(crate) fn build_page(&mut self) -> Result<(), Jump> {
        if self.level_list_is_empty(0) || self.output_active() {
            return Ok(());
        }
        loop {
            // (the contributions wait here while the steps run: a step
            // reads its node and, for a kern, what follows it)
            let mut rest: VecDeque<Node> = mem::take(self.contrib()).into_vec().into();
            let mut fired = None;
            let r = loop {
                let Some(p) = rest.pop_front() else {
                    break Ok(());
                };
                // (what follows is read for a kern only, §1000)
                let after = if matches!(p, Node::Kern { .. }) {
                    After::of(rest.front())
                } else {
                    After::Other
                };
                match self.page_step(p, after) {
                    Err(j) => break Err(j),
                    Ok(s) => {
                        // §998, §999
                        if let Some(p) = s.page {
                            self.page_push(p);
                        }
                        if let Some(p) = s.discard {
                            self.page_discards_mut().push(p);
                        }
                        for n in s.front.into_iter().rev() {
                            rest.push_front(n);
                        }
                        match s.then {
                            Then::Next => {}
                            Then::Return => break Ok(()),
                            Then::Fired(classes) => {
                                fired = Some(classes);
                                break Ok(());
                            }
                            Then::Defer => {
                                self.fire_pending = true;
                                break Ok(());
                            }
                        }
                    }
                }
            };
            *self.contrib() = Vec::from(rest).into();
            r?;
            let Some(classes) = fired else {
                return Ok(());
            };
            self.output_routine(&classes)?;
            if self.output_active() {
                return Ok(()); // user's output routine will act
            }
            // the page has been shipped out by default output routine
            if self.level_list_is_empty(0) {
                return Ok(());
            }
        }
    }

    /// §1005's `fire_up(p)`, deferred to the step that begins at it
    /// (DESIGN §7.16.1, "The deferred fire's form"), its first action:
    /// the fire on the first contribution, then the output routine, or
    /// the default one and the rest of `build_page`'s loop, as §1005
    /// goes on after it.
    pub(crate) fn fire_deferred(&mut self) -> Result<(), Jump> {
        self.fire_pending = false;
        // (the window that begins with the fire ends at the first boundary
        // after its output routine: DESIGN 4.3 item 1)
        self.window_event(crate::run::WindowEvent::Fire);
        let mut rest: VecDeque<Node> = mem::take(self.contrib()).into_vec().into();
        let Some(p) = rest.pop_front() else {
            return self.confusion(b"fire");
        };
        let mut front = vec![p];
        let r = self.fire_up(&mut front);
        for n in front.into_iter().rev() {
            rest.push_front(n);
        }
        *self.contrib() = Vec::from(rest).into();
        let classes = r?;
        self.output_routine(&classes)?;
        if self.output_active() {
            return Ok(()); // user's output routine will act
        }
        // the page has been shipped out by default output routine
        self.build_page()
    }

    /// §996–§1008, and §1012–§1022 when the page is complete: one step of
    /// `build_page`, node `p`, the first contribution. A recorded call
    /// (`Func::PageStep`, DESIGN 7.17.2) named by what it reads of the
    /// page so far on every path ([`step_name`]), `p`, and `after` (what
    /// follows a kern). It reads the page's fields and the parameters as
    /// TeX's step does. Its writes are the fields it assigns, and its
    /// result what goes to the page or back in front of the contributions
    /// (`PAGE_STEP_RESULT`): the contributions are not its to read.
    fn page_step(&mut self, p: Node, after: After) -> Result<Stepped, Jump> {
        if !T::VALUES {
            return self.page_step_body(p, after, None, 0);
        }
        let before = self.page_fields();
        let mask = step_name(&p, after, &self.page);
        let page: Vec<Version> = (0..page_row::COUNT)
            .filter(|f| mask & 1 << f != 0)
            .map(|f| Version(before[usize::from(f)]))
            .collect();
        let name = Version::node(
            0x7073_7465,
            &[
                Version::node(0x7061_6765, &page),
                Version::of(&p),
                Version::of(&after),
            ],
        );
        self.tracker.call_begin(Func::PageStep, &[name.0], self);
        let r = self.page_step_body(p, after, Some(&before), mask);
        if let Ok(s) = &r {
            self.tracker
                .row_wrote(Row::Scalar(scalar::PAGE_STEP_RESULT), Version::of(s).0);
        }
        self.tracker.call_end(self);
        r
    }

    fn page_step_body(
        &mut self,
        p: Node,
        after: After,
        before: Option<&PageFields>,
        name: u32,
    ) -> Result<Stepped, Jump> {
        let mut page = self.page_take();
        let mut events = Vec::new();
        let mut acc = builder::Access::default();
        let step = page.step(p, after, &*self, &mut events, &mut acc);
        // (the name's fields are reads, or the fire reads them)
        debug_assert!(
            !matches!(
                step,
                Ok(Step::Page(_) | Step::Discard(_) | Step::TopSkip(..) | Step::Kern(_))
            ) || (0..32)
                .filter(|f| name & 1 << f != 0)
                .all(|f| acc.was_read(f)),
            "a page step's name reads more than the step: {name:#x}, {acc:?}"
        );
        self.page_put(page, acc, before);
        self.print_page_events(events)?;
        let mut s = Stepped {
            page: None,
            discard: None,
            front: Vec::new(),
            then: Then::Next,
        };
        match step {
            Err(c) => return self.confusion(c.0.as_bytes()),
            Ok(Step::Page(p)) => s.page = Some(p),
            Ok(Step::Discard(p)) => s.discard = p,
            Ok(Step::TopSkip(glue, p)) => s.front = vec![glue, p],
            Ok(Step::Kern(p)) => {
                s.front = vec![p];
                s.then = Then::Return;
            }
            Ok(Step::FireUp(p)) if self.deferring_fire() => {
                // §1005, deferred: `p` stays the first contribution
                s.front = vec![p];
                s.then = Then::Defer;
            }
            Ok(Step::FireUp(p)) => {
                // §1005: `fire_up(p)`
                s.front = vec![p];
                let classes = self.fire_up(&mut s.front)?;
                s.then = Then::Fired(classes);
            }
        }
        Ok(s)
    }

    /// Print what the page builder reported, in order.
    fn print_page_events(&mut self, events: Vec<Event>) -> Result<(), Jump> {
        for e in events {
            match e {
                Event::Freeze { goal, max_depth } => {
                    // §987
                    self.begin_diagnostic();
                    self.print_nl(b"%% goal height=");
                    self.print_scaled(goal);
                    self.print_str(b", max depth=");
                    self.print_scaled(max_depth);
                    self.end_diagnostic(false);
                }
                Event::Cost {
                    so_far,
                    b,
                    pi,
                    c,
                    best,
                } => {
                    // §1006: display the page break cost.
                    self.begin_diagnostic();
                    self.print_nl(b"%");
                    self.print_str(b" t=");
                    self.print_totals(&so_far);
                    self.print_str(b" g=");
                    self.print_scaled(so_far[0]);
                    self.print_str(b" b=");
                    if b == AWFUL_BAD {
                        self.print_char(b'*');
                    } else {
                        self.print_int(b);
                    }
                    self.print_str(b" p=");
                    self.print_int(pi);
                    self.print_str(b" c=");
                    if c == AWFUL_BAD {
                        self.print_char(b'*');
                    } else {
                        self.print_int(c);
                    }
                    if best {
                        self.print_char(b'#');
                    }
                    self.end_diagnostic(false);
                }
                Event::InfiniteShrinkPage => {
                    // §1004
                    self.print_err(b"Infinite glue shrinkage found on current page");
                    self.help(&[
                        b"The page about to be output contains some infinitely",
                        b"shrinkable glue, e.g., `\\vss' or `\\vskip 0pt minus 1fil'.",
                        b"Such glue doesn't belong there; but you can safely proceed,",
                        b"since the offensive shrinkability has been made finite.",
                    ]);
                    self.error()?;
                }
                Event::NotVbox(n) => self.ensure_vbox(i32::from(n))?,
                Event::InfiniteShrinkSkip(n) => {
                    // §1009
                    self.print_err(b"Infinite glue shrinkage inserted from ");
                    self.print_esc(b"skip");
                    self.print_int(i32::from(n));
                    self.help(&[
                        b"The correction glue for page breaking with insertions",
                        b"must have finite shrinkability. But you may proceed,",
                        b"since the offensive shrinkability has been made finite.",
                    ]);
                    self.error()?;
                }
                Event::InfiniteShrinkSplit => self.infinite_shrink_split_error()?,
                Event::Split {
                    n,
                    w,
                    height_plus_depth,
                    pi,
                } => {
                    // §1011: display the insertion split cost.
                    self.begin_diagnostic();
                    self.print_nl(b"% split");
                    self.print_int(i32::from(n));
                    self.print_str(b" to ");
                    self.print_scaled(w);
                    self.print_char(b',');
                    self.print_scaled(height_plus_depth);
                    self.print_str(b" p=");
                    self.print_int(pi);
                    self.end_diagnostic(false);
                }
            }
        }
        Ok(())
    }

    /// §1012–§1022: cut the current page at the best place into box 255,
    /// fill the insertion boxes, and put the rest of the page in front of
    /// `front` (the break node, first of the contributions). The classes
    /// of the insertion boxes it filled, for the output routine's call.
    fn fire_up(&mut self, front: &mut Vec<Node>) -> Result<Vec<u8>, Jump> {
        // §1015: ensure that box 255 is empty before output.
        if self.box_reg(255).is_some() {
            self.print_err(b"");
            self.print_esc(b"box");
            self.print_str(b"255 is not void");
            self.help(&[
                b"You shouldn't use \\box255 except in \\output routines.",
                b"Proceed, and I'll discard its present contents.",
            ]);
            self.box_error(255)?;
        }
        let holding = self.int_par(HOLDING_INSERTS_CODE) > 0;
        // §1018: the boxes involved in insertions act as queues.
        let mut queues: Vec<(u8, Option<BoxNode>)> = Vec::new();
        if !holding {
            for i in 0..self.page_ins().len() {
                let r = self.page_ins()[i];
                if r.best_ins.is_some() {
                    let n = i32::from(r.number);
                    self.ensure_vbox(n)?;
                    let b = self.take_box(n).map(Arc::unwrap_or_clone);
                    queues.push((r.number, b));
                }
            }
        }
        for c in self
            .cur_mark
            .keys()
            .copied()
            .collect::<alloc::vec::Vec<_>>()
        {
            for t in [TOP_MARK_I, FIRST_MARK_I, BOT_MARK_I] {
                self.mark_read(c, t);
            }
        }
        let mut marks: MarkClasses = self
            .cur_mark
            .iter_mut()
            .map(|(&c, m)| {
                (
                    c,
                    Marks {
                        top: m[TOP_MARK].take(),
                        first: m[FIRST_MARK].take(),
                        bot: m[BOT_MARK].take(),
                    },
                )
            })
            .collect();
        let mut page = mem::take(self.page_all_mut());
        // (§1021's packs are `vpack` calls; a confusion in one is kept
        // as the jump it raised, printed where it happened)
        let mut jump = None;
        let fired = page.fire_up(
            front,
            holding,
            &mut marks,
            |n| {
                queues
                    .iter_mut()
                    .find(|(m, _)| *m == n)
                    .and_then(|(_, b)| b.take())
            },
            |list| match self.vpack(list, Spec::NATURAL) {
                Ok(b) => Ok(b),
                Err(j) => {
                    jump = Some(j);
                    Err(partex_engine::pack::Confusion("vpack"))
                }
            },
        );
        self.page = page;
        self.page_list_wrote();
        for (c, m) in marks {
            let cur = self.cur_mark.entry(c).or_default();
            cur[TOP_MARK] = m.top;
            cur[FIRST_MARK] = m.first;
            cur[BOT_MARK] = m.bot;
            for t in [TOP_MARK_I, FIRST_MARK_I, BOT_MARK_I] {
                self.mark_wrote(c, t);
            }
        }
        if let Some(j) = jump {
            return Err(j);
        }
        let fired = match fired {
            Ok(f) => f,
            Err(c) => return self.confusion(c.0.as_bytes()),
        };
        // (§1018's errors, before the page is packed)
        self.print_page_events(fired.events)?;
        // §1017: box 255, its report inhibited
        let page = self
            .vpack_quiet(fired.page, Spec::Exactly(fired.size), fired.max_depth)?
            .node;
        // §1013
        self.geq_word_define(INT_BASE + OUTPUT_PENALTY_CODE, fired.output_penalty);
        let classes = fired.boxes.iter().map(|(n, _)| *n).collect();
        for (n, b) in fired.boxes {
            self.set_box_reg(i32::from(n), Some(b.share()));
        }
        self.set_box_reg(255, Some(page.share()));
        Ok(classes)
    }

    /// §1012's end: the output routine, the user's (§1025, ended by
    /// §1026 in [`Self::output_end`]) or the default one (§1024,
    /// §1023). A recorded call (`Func::Output`, DESIGN 7.17.4) over
    /// `\box255`, the insertion boxes of `classes` and
    /// `\outputpenalty`; it reads `\output` and `\deadcycles` inside.
    fn output_routine(&mut self, classes: &[u8]) -> Result<(), Jump> {
        if T::VALUES {
            let mut parts = vec![
                Version::of(&self.box_reg(255)),
                Version::of(&self.int_par(OUTPUT_PENALTY_CODE)),
            ];
            for &n in classes {
                parts.push(Version::of(&(n, self.box_reg(i32::from(n)))));
            }
            let name = Version::node(0x6f75_7470, &parts);
            self.tracker.call_begin(Func::Output, &[name.0], self);
        }
        let user = self.equiv_toks(OUTPUT_ROUTINE_LOC).is_some();
        if user && self.dead_cycles() < self.int_par(MAX_DEAD_CYCLES_CODE) {
            // §1025: fire up the user's output routine and `return` (the
            // call stays open until §1026)
            self.set_output_active(true);
            self.set_dead_cycles(self.dead_cycles() + 1);
            self.push_nest()?;
            self.set_mode(-VMODE);
            self.set_prev_depth(IGNORE_DEPTH);
            self.set_ml(-self.line);
            self.begin_toks_at(OUTPUT_ROUTINE_LOC, OUTPUT_TEXT)?;
            self.new_save_level(OUTPUT_GROUP)?;
            self.normal_paragraph()?;
            return self.scan_left_brace();
        }
        let r = self.default_output(user);
        if T::VALUES {
            self.tracker.call_end(self);
        }
        r
    }

    /// §1026's end: the user's output routine's call ends, its result
    /// `held`, what goes in front of the contributions.
    pub(crate) fn output_end(&self, held: &partex_engine::nodelist::NodeList) {
        if T::VALUES {
            self.tracker
                .row_wrote(Row::Scalar(scalar::OUTPUT_RESULT), held.version().0);
            self.tracker.call_end(self);
        }
    }

    /// §1024 (after the user's routine looped `\maxdeadcycles` times)
    /// and §1023: the default output routine.
    fn default_output(&mut self, looped: bool) -> Result<(), Jump> {
        if looped {
            // §1024: explain that too many dead cycles have occurred in a
            // row.
            self.print_err(b"Output loop---");
            self.print_int(self.dead_cycles());
            self.print_str(b" consecutive dead cycles");
            self.help(&[
                b"I've concluded that your \\output is awry; it never does a",
                b"\\shipout, so I'm shipping \\box255 out myself. Next time",
                b"increase \\maxdeadcycles if you want me to be more patient!",
            ]);
            self.error()?;
        }
        // §1023: perform the default output routine.
        let mut held = self.page_take_list();
        if !held.is_empty() {
            let contrib = self.contrib();
            held.append(contrib);
            *contrib = held;
        }
        self.page_discards_mut().clear();
        if let Some(b) = self.box_reg(255).cloned() {
            self.ship_out(&b)?;
        }
        self.set_box_reg(255, None);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::FIL_GLUE;
    use crate::nodes::new_glue;
    use crate::testing::{engine, term_output};
    use partex_engine::builder::Builder;
    use partex_engine::pack::Spec;

    /// `testdata/page.tex` under `tex -ini -output-comment=partex`: each
    /// contribution is followed by `build_page`, as main control does in
    /// vertical mode; the `\tracingpages`/`\tracingoutput` transcript and
    /// the DVI file must match.
    #[test]
    fn build_page_matches_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        t.init_null_font();
        t.params.output_comment = Some(b"partex".to_vec());
        t.job_name = t.make_tex_string(b"pg").unwrap();
        t.page = Builder::default();
        let pt = 65536;
        for (code, v) in [
            (SHOW_BOX_DEPTH_CODE, 100),
            (SHOW_BOX_BREADTH_CODE, 100),
            (TRACING_PAGES_CODE, 1),
            (TRACING_ONLINE_CODE, 1),
            (TRACING_OUTPUT_CODE, 1),
        ] {
            t.set_int_par(code, v);
        }
        t.set_eqtb_int(DIMEN_BASE + VSIZE_CODE, 50 * pt);
        t.set_eqtb_int(DIMEN_BASE + MAX_DEPTH_CODE, 2 * pt);
        for (code, w) in [(TOP_SKIP_CODE, 10 * pt), (BASELINE_SKIP_CODE, 12 * pt)] {
            let g = t.new_glue_value(
                GlueSpec {
                    width: w,
                    ..GlueSpec::default()
                },
                None,
            );
            let word = t.peek_eqtb(GLUE_BASE + code);
            t.set_eqtb_entry(GLUE_BASE + code, word, Some(crate::objs::Obj::Glue(g)));
        }
        t.set_prev_depth(IGNORE_DEPTH);
        let boxed = |t: &mut Tex<_, _>| {
            let r = Node::Rule {
                width: 26214, // `default_rule`
                height: 7 * pt,
                depth: 2 * pt,
                sync: partex_engine::origin::Side(0),
            };
            let b = t.hpack(
                alloc::vec![r, new_glue(FIL_GLUE)],
                Spec::Exactly(20 * pt),
                None,
            );
            t.append_to_vlist(Node::Box(b.share()));
            t.build_page().unwrap();
        };
        let penalty = |t: &mut Tex<_, _>, n| {
            t.tail_append(Node::Penalty(n));
            t.build_page().unwrap();
        };
        let out = term_output(&mut t, |t| {
            for _ in 0..3 {
                boxed(t);
            }
            penalty(t, 50);
            for _ in 0..4 {
                boxed(t);
            }
            penalty(t, -10000);
            t.finish_dvi_file().unwrap();
        });
        let out = alloc::string::String::from_utf8(out).unwrap();
        let (trace, _) = out.split_once("\nOutput written").unwrap();
        assert_eq!(
            trace.trim_matches('\n'),
            include_str!("../testdata/page.log").trim_matches('\n')
        );
        assert_eq!(
            t.host.written[&1].as_slice(),
            include_bytes!("../testdata/page.dvi")
        );
    }
}
