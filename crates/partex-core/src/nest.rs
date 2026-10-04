//! Part 16: The semantic nest (§211–§219).

use alloc::boxed::Box;
use alloc::vec::Vec;

use partex_engine::Scaled;
use partex_engine::math::{Item, Noad};
use partex_engine::node::Node;

use crate::cmds::{MAX_COMMAND, MMODE, VMODE};
use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// §212: `prev_depth` value that is ignored.
pub const IGNORE_DEPTH: i32 = -65_536_000;

/// §212: the state of one semantic level.
///
/// tex.web's `head`/`tail` list is `list` in vertical and horizontal
/// modes and `mlist` in math modes; `aux` is split into its meanings
/// (each mode sets the one it uses when the level begins, so no value
/// is ever read through another's name).
#[derive(Clone, Debug, Default, Hash)]
pub struct ListStateRecord {
    pub mode: i32,
    /// The list: a persistent sequence of nodes (`nodelist.rs`), an
    /// append sharing the prefix.
    pub list: partex_engine::nodelist::NodeList,
    pub mlist: Vec<Item>,
    /// `prev_graf` (`pg_field`).
    pub pg: i32,
    /// `mode_line` (`ml_field`).
    pub ml: i32,
    pub prev_depth: Scaled,
    pub space_factor: i32,
    pub clang: i32,
    /// `incompleat_noad`: the fraction whose numerator is being built.
    pub incompleat: Option<Box<Noad>>,
    /// e-TeX: a math left group's list began at `\middle` (not `\left`).
    pub middle: bool,
    /// `TeXXeT`, in a vertical list: the text-direction segments left open
    /// by a paragraph a display interrupted (`LR_save`).
    pub lr_save: Vec<u8>,
    /// e-TeX, in display math: the prototype box of the display lines
    /// (`LR_box`).
    pub lr_box: Option<partex_engine::node::BoxNode>,
}

partex_engine::persist_struct!(ListStateRecord {
    mode,
    list,
    mlist,
    pg,
    ml,
    prev_depth,
    space_factor,
    clang,
    incompleat,
    middle,
    lr_save,
    lr_box
});

/// The version of level `l`'s field `f` (made from the value: the list
/// carries its own; a math list, an incomplete noad and the e-TeX fields,
/// rare at a call's boundary, by their contents).
pub(crate) fn field_version(l: &ListStateRecord, f: u8) -> u128 {
    use crate::track::list::*;
    let int = |v: i32| crate::track::scalar_version_i32(v);
    match f {
        LIST => l.list.version().0,
        MLIST => partex_ssa::Version::of(&l.mlist).0,
        MODE => int(l.mode),
        PG => int(l.pg),
        ML => int(l.ml),
        PREV_DEPTH => int(l.prev_depth),
        SPACE_FACTOR => int(l.space_factor),
        CLANG => int(l.clang),
        INCOMPLEAT => partex_ssa::Version::of(&l.incompleat).0,
        MIDDLE => int(i32::from(l.middle)),
        LR_SAVE => partex_ssa::Version::of(&l.lr_save).0,
        _ => partex_ssa::Version::of(&l.lr_box).0,
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    // §213: the fields of each level of the nest as values (DESIGN
    // 7.17.12, the `cur_list` row), by depth (`track::list::slot`): each
    // read tells the tracker the field's version, each change is a write
    // of it; the list is a persistent sequence that carries its version,
    // the scalars are versioned by their values. A level's fields stay in
    // their slots while levels above it come and go: entering one reads
    // only what the new level takes from it, and leaving one writes
    // nothing of the level left to (a box built in vertical mode does not
    // read the `\prevgraf` of the paragraph before it).
    #[inline]
    fn list_read(&self, f: u8) {
        if T::VALUES {
            // (which level is current reads the nest's depth)
            self.nest_read();
            self.tracker.value_read(
                crate::track::Row::List(crate::track::list::slot(self.nest.len(), f)),
                || self.list_field_version(f),
            );
        }
    }

    #[inline]
    fn list_wrote(&self, f: u8) {
        if T::VALUES {
            self.nest_read();
            self.tracker
                .value_wrote(crate::track::Row::List(crate::track::list::slot(
                    self.nest.len(),
                    f,
                )));
        }
    }

    /// Field `f` of level `p` (below `cur_list`, or `cur_list` itself)
    /// read.
    fn level_read(&self, p: usize, f: u8) {
        if p >= self.nest.len() {
            self.list_read(f);
        } else if T::VALUES {
            self.tracker.value_read(
                crate::track::Row::List(crate::track::list::slot(p, f)),
                || field_version(&self.nest[p], f),
            );
        }
    }

    /// Field `f` of level `p` (below `cur_list`, or `cur_list` itself)
    /// written.
    fn level_field_wrote(&self, p: usize, f: u8) {
        if p >= self.nest.len() {
            self.list_wrote(f);
        } else if T::VALUES {
            self.tracker
                .value_wrote(crate::track::Row::List(crate::track::list::slot(p, f)));
        }
    }

    /// All of `cur_list`'s fields and the nest written (a level entered,
    /// §216).
    fn level_wrote(&self) {
        if T::VALUES {
            // (the nest first: the fields' slots read its depth, the new
            // one)
            self.tracker.value_wrote(crate::track::Row::Nest);
            for f in 0..crate::track::list::COUNT {
                self.list_wrote(f);
            }
        }
    }

    /// The version of `cur_list`'s field `f` now ([`field_version`]).
    pub(crate) fn list_field_version(&self, f: u8) -> u128 {
        field_version(&self.cur_list, f)
    }

    /// The version of field `f` of the level at slot `i` (`track::list::
    /// slot`), or none if there is no such level now.
    pub(crate) fn level_field_version(&self, i: u32) -> Option<u128> {
        let (d, f) = (
            usize::try_from(i / crate::track::list::STRIDE).ok()?,
            u8::try_from(i % crate::track::list::STRIDE).ok()?,
        );
        let l = match d.cmp(&self.nest.len()) {
            core::cmp::Ordering::Less => &self.nest[d],
            core::cmp::Ordering::Equal => &self.cur_list,
            core::cmp::Ordering::Greater => return None,
        };
        Some(field_version(l, f))
    }

    /// The level at depth `d`, `cur_list` the deepest.
    pub(crate) fn level_at_depth(&self, d: usize) -> Option<&ListStateRecord> {
        match d.cmp(&self.nest.len()) {
            core::cmp::Ordering::Less => self.nest.get(d),
            core::cmp::Ordering::Equal => Some(&self.cur_list),
            core::cmp::Ordering::Greater => None,
        }
    }

    /// The level at depth `d`, to change.
    pub(crate) fn level_at_depth_mut(&mut self, d: usize) -> Option<&mut ListStateRecord> {
        match d.cmp(&self.nest.len()) {
            core::cmp::Ordering::Less => self.nest.get_mut(d),
            core::cmp::Ordering::Equal => Some(&mut self.cur_list),
            core::cmp::Ordering::Greater => None,
        }
    }

    /// The version of the nest: its depth (the levels' fields are slots
    /// of their own, by depth).
    pub(crate) fn nest_version(&self) -> u128 {
        crate::track::scalar_version_i32(i32::try_from(self.nest.len()).unwrap_or(i32::MAX))
    }

    fn nest_read(&self) {
        if T::VALUES {
            self.tracker
                .value_read(crate::track::Row::Nest, || self.nest_version());
        }
    }

    fn nest_wrote(&self) {
        if T::VALUES {
            self.tracker.value_wrote(crate::track::Row::Nest);
        }
    }

    pub(crate) fn mode(&self) -> i32 {
        self.list_read(crate::track::list::MODE);
        self.cur_list.mode
    }
    pub(crate) fn set_mode(&mut self, v: i32) {
        self.cur_list.mode = v;
        self.list_wrote(crate::track::list::MODE);
    }
    pub(crate) fn pg(&self) -> i32 {
        self.list_read(crate::track::list::PG);
        self.cur_list.pg
    }
    pub(crate) fn set_pg(&mut self, v: i32) {
        self.cur_list.pg = v;
        self.list_wrote(crate::track::list::PG);
    }
    pub(crate) fn ml(&self) -> i32 {
        self.list_read(crate::track::list::ML);
        self.cur_list.ml
    }
    pub(crate) fn set_ml(&mut self, v: i32) {
        self.cur_list.ml = v;
        self.list_wrote(crate::track::list::ML);
    }
    pub(crate) fn middle(&self) -> bool {
        self.list_read(crate::track::list::MIDDLE);
        self.cur_list.middle
    }
    pub(crate) fn set_middle(&mut self, v: bool) {
        self.cur_list.middle = v;
        self.list_wrote(crate::track::list::MIDDLE);
    }
    pub(crate) fn prev_depth(&self) -> i32 {
        self.list_read(crate::track::list::PREV_DEPTH);
        self.cur_list.prev_depth
    }
    pub(crate) fn set_prev_depth(&mut self, v: i32) {
        self.cur_list.prev_depth = v;
        self.list_wrote(crate::track::list::PREV_DEPTH);
    }
    pub(crate) fn space_factor(&self) -> i32 {
        self.list_read(crate::track::list::SPACE_FACTOR);
        self.cur_list.space_factor
    }
    pub(crate) fn set_space_factor(&mut self, v: i32) {
        self.cur_list.space_factor = v;
        self.list_wrote(crate::track::list::SPACE_FACTOR);
    }
    pub(crate) fn clang(&self) -> i32 {
        self.list_read(crate::track::list::CLANG);
        self.cur_list.clang
    }
    pub(crate) fn set_clang(&mut self, v: i32) {
        self.cur_list.clang = v;
        self.list_wrote(crate::track::list::CLANG);
    }
    /// The current list, read.
    pub(crate) fn nodes(&self) -> &partex_engine::nodelist::NodeList {
        self.list_read(crate::track::list::LIST);
        &self.cur_list.list
    }
    /// The current list, to change (a read of it and a write).
    pub(crate) fn nodes_mut(&mut self) -> &mut partex_engine::nodelist::NodeList {
        self.list_read(crate::track::list::LIST);
        self.list_wrote(crate::track::list::LIST);
        &mut self.cur_list.list
    }
    pub(crate) fn mlist(&self) -> &Vec<Item> {
        self.list_read(crate::track::list::MLIST);
        &self.cur_list.mlist
    }
    pub(crate) fn mlist_mut(&mut self) -> &mut Vec<Item> {
        self.list_read(crate::track::list::MLIST);
        self.list_wrote(crate::track::list::MLIST);
        &mut self.cur_list.mlist
    }
    pub(crate) fn incompleat(&self) -> Option<&Noad> {
        self.list_read(crate::track::list::INCOMPLEAT);
        self.cur_list.incompleat.as_deref()
    }
    pub(crate) fn incompleat_mut(&mut self) -> &mut Option<Box<Noad>> {
        self.list_read(crate::track::list::INCOMPLEAT);
        self.list_wrote(crate::track::list::INCOMPLEAT);
        &mut self.cur_list.incompleat
    }
    pub(crate) fn lr_save(&self) -> &Vec<u8> {
        self.list_read(crate::track::list::LR_SAVE);
        &self.cur_list.lr_save
    }
    pub(crate) fn lr_save_mut(&mut self) -> &mut Vec<u8> {
        self.list_read(crate::track::list::LR_SAVE);
        self.list_wrote(crate::track::list::LR_SAVE);
        &mut self.cur_list.lr_save
    }
    pub(crate) fn lr_box_mut(&mut self) -> &mut Option<partex_engine::node::BoxNode> {
        self.list_read(crate::track::list::LR_BOX);
        self.list_wrote(crate::track::list::LR_BOX);
        &mut self.cur_list.lr_box
    }

    /// §213: `nest_ptr`: the number of enclosing levels.
    pub(crate) fn nest_ptr(&self) -> usize {
        self.nest_read();
        self.nest.len()
    }

    /// Level `p` of the nest (`nest_ptr` is `cur_list`), whole: its
    /// caller reads any field. (Which level `p` is reads the nest's depth.)
    pub(crate) fn nest_at(&self, p: usize) -> &ListStateRecord {
        self.nest_read();
        for f in 0..crate::track::list::COUNT {
            self.level_read(p, f);
        }
        self.nest.get(p).unwrap_or(&self.cur_list)
    }

    /// Field `f` of level `p` read alone, and the level.
    fn level_field(&self, p: usize, f: u8) -> &ListStateRecord {
        self.nest_read();
        self.level_read(p, f);
        self.nest.get(p).unwrap_or(&self.cur_list)
    }

    /// The mode of level `p`.
    pub(crate) fn level_mode(&self, p: usize) -> i32 {
        self.level_field(p, crate::track::list::MODE).mode
    }

    /// The `prev_graf` of level `p`.
    pub(crate) fn level_pg(&self, p: usize) -> i32 {
        self.level_field(p, crate::track::list::PG).pg
    }

    /// The `prev_graf` of level `p` set (§1244).
    pub(crate) fn set_level_pg(&mut self, p: usize, v: i32) {
        self.nest_read();
        self.level_field_wrote(p, crate::track::list::PG);
        match self.nest.get_mut(p) {
            Some(l) => l.pg = v,
            None => self.cur_list.pg = v,
        }
    }

    /// The `prev_depth` of level `p`.
    pub(crate) fn level_prev_depth(&self, p: usize) -> i32 {
        self.level_field(p, crate::track::list::PREV_DEPTH)
            .prev_depth
    }

    /// Whether level `p`'s math list was begun by `\middle`.
    pub(crate) fn level_middle(&self, p: usize) -> bool {
        self.level_field(p, crate::track::list::MIDDLE).middle
    }

    /// Whether level `p`'s list is empty (level 0's: the contribution
    /// list).
    pub(crate) fn level_list_is_empty(&self, p: usize) -> bool {
        self.level_field(p, crate::track::list::LIST)
            .list
            .is_empty()
    }

    /// The contribution list (§215: the outer level's list). (Whether
    /// there is an outer level reads the nest: a step that found it
    /// empty and took `cur_list`'s list reads the nest too, or a rebuild
    /// does not predict the read and places `cur_list` over another
    /// nest.)
    pub(crate) fn contrib(&mut self) -> &mut partex_engine::nodelist::NodeList {
        self.nest_read();
        if self.nest.is_empty() {
            return self.nodes_mut();
        }
        self.level_read(0, crate::track::list::LIST);
        self.level_field_wrote(0, crate::track::list::LIST);
        &mut self.nest[0].list
    }

    /// Is the current list empty (`head=tail`)?
    pub(crate) fn list_is_empty(&self) -> bool {
        self.nodes().is_empty() && self.mlist().is_empty()
    }

    /// §214: `tail_append(p)`: a node goes to the current list (an mlist
    /// in math mode).
    pub(crate) fn tail_append(&mut self, p: Node) {
        if self.mode().abs() == MMODE {
            self.mlist_mut().push(Item::Node(p));
        } else {
            self.nodes_mut().push(p);
        }
    }

    /// The last entry of the current list, if it is a node.
    pub(crate) fn tail_item(&self) -> Option<&Node> {
        if self.mode().abs() == MMODE {
            match self.mlist().last() {
                Some(Item::Node(n)) => Some(n),
                _ => None,
            }
        } else {
            self.nodes().last()
        }
    }

    /// tex.web's `tail`, when it is a node and the list is not empty: the
    /// last node, or the last node a final discretionary replaces (which
    /// tex.web keeps after the discretionary).
    pub(crate) fn tail_node(&self) -> Option<&Node> {
        match self.tail_item() {
            Some(Node::Disc(d)) if !d.replace.is_empty() => d.replace.last(),
            n => n,
        }
    }

    /// Remove the last node of the current list (it must be a node).
    pub(crate) fn pop_tail(&mut self) -> Option<Node> {
        if self.mode().abs() == MMODE {
            let m = self.mlist_mut();
            match m.pop() {
                Some(Item::Node(n)) => Some(n),
                Some(i) => {
                    m.push(i);
                    None
                }
                None => None,
            }
        } else {
            self.nodes_mut().pop()
        }
    }

    /// §211: print the mode represented by `m`.
    pub(crate) fn print_mode(&mut self, m: i32) {
        let s: &[u8] = if m > 0 {
            match m / (MAX_COMMAND + 1) {
                0 => b"vertical mode",
                1 => b"horizontal mode",
                2 => b"display math mode",
                _ => b"",
            }
        } else if m == 0 {
            b"no mode"
        } else {
            match (-m) / (MAX_COMMAND + 1) {
                0 => b"internal vertical mode",
                1 => b"restricted horizontal mode",
                2 => b"math mode",
                _ => b"",
            }
        };
        self.print_str(s);
    }

    /// §211 (web2c): print "' in <mode>".
    pub(crate) fn print_in_mode(&mut self, m: i32) {
        let s: &[u8] = if m > 0 {
            match m / (MAX_COMMAND + 1) {
                0 => b"' in vertical mode",
                1 => b"' in horizontal mode",
                2 => b"' in display math mode",
                _ => b"",
            }
        } else if m == 0 {
            b"' in no mode"
        } else {
            match (-m) / (MAX_COMMAND + 1) {
                0 => b"' in internal vertical mode",
                1 => b"' in restricted horizontal mode",
                2 => b"' in math mode",
                _ => b"",
            }
        };
        self.print_str(s);
    }

    /// §215: the semantic nest part of "set initial values". The page
    /// builder variables (a copy of §991) are set with part 45.
    pub(crate) fn init_nest(&mut self) {
        self.nest.clear();
        self.max_nest_stack = 0;
        self.cur_list = ListStateRecord::default();
        self.cur_list.mode = VMODE;
        self.cur_list.prev_depth = IGNORE_DEPTH;
        self.cur_list.ml = 0;
        self.cur_list.pg = 0;
        self.level_wrote();
        self.set_shown_mode(0);
    }

    /// §216: enter a new semantic level, saving the old.
    pub(crate) fn push_nest(&mut self) -> Result<(), Jump> {
        let nest_ptr = self.nest_ptr();
        // (the size tested apart from the statistic, which a dropped run
        // leaves at its deepest)
        let size = usize::try_from(self.params.nest_size).unwrap_or(0);
        if nest_ptr > self.max_nest_stack || nest_ptr >= size {
            self.max_nest_stack = self.max_nest_stack.max(nest_ptr);
            if nest_ptr >= size {
                let n = self.params.nest_size;
                return self.overflow(b"semantic nest size", n);
            }
        }
        // (the new level begins with the old one's mode and `aux` (§216),
        // which are read; the old level's other fields stay in its slots)
        {
            use crate::track::list::{CLANG, MODE, PREV_DEPTH, SPACE_FACTOR};
            for f in [MODE, PREV_DEPTH, SPACE_FACTOR, CLANG] {
                self.list_read(f);
            }
        }
        let new = ListStateRecord {
            mode: self.cur_list.mode,
            pg: 0,
            ml: self.line,
            prev_depth: self.cur_list.prev_depth,
            space_factor: self.cur_list.space_factor,
            clang: self.cur_list.clang,
            // most lists get a few nodes: skip the first doublings
            list: partex_engine::nodelist::NodeList::new(),
            ..ListStateRecord::default()
        };
        let old = core::mem::replace(&mut self.cur_list, new);
        self.nest.push(old); // stack the record
        self.level_wrote();
        Ok(())
    }

    /// §217: leave a semantic level, re-entering the old. The current
    /// lists are dropped (callers take what they keep first).
    pub(crate) fn pop_nest(&mut self) {
        self.nest_read();
        self.cur_list = self.nest.pop().expect("semantic nest");
        // (the level left to is in its slots as it was)
        self.nest_wrote();
    }
}
