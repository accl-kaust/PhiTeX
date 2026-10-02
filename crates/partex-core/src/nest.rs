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

impl<H: Host, T: Tracker> Tex<H, T> {
    // §213: the fields of `cur_list` as values (DESIGN 7.17.12, the
    // `cur_list` row): each read tells the tracker the field's version,
    // each change is a write of it; the list is a persistent sequence that
    // carries its version, the scalars are versioned by their values.
    #[inline]
    fn list_read(&self, f: u8) {
        if T::VALUES {
            self.tracker
                .value_read(crate::track::Row::List(f), || self.list_field_version(f));
        }
    }

    #[inline]
    fn list_wrote(&self, f: u8) {
        if T::VALUES {
            self.tracker.value_wrote(crate::track::Row::List(f));
        }
    }

    /// All of `cur_list`'s fields and the nest written (a level entered or
    /// left, §216, §217).
    fn level_wrote(&self) {
        if T::VALUES {
            for f in 0..crate::track::list::COUNT {
                self.list_wrote(f);
            }
            self.tracker.value_wrote(crate::track::Row::Nest);
        }
    }

    /// The version of `cur_list`'s field `f` now (made from the value: the
    /// list carries its own; a math list, an incomplete noad and the
    /// e-TeX fields, rare at a call's boundary, by their contents).
    pub(crate) fn list_field_version(&self, f: u8) -> u128 {
        use crate::track::list::*;
        let l = &self.cur_list;
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

    /// The version of the enclosing levels (the nest below `cur_list`):
    /// each level's fields, its list by the version it carries.
    pub(crate) fn nest_version(&self) -> u128 {
        let mut h = partex_engine::stablehash::StableHasher::new();
        core::hash::Hash::hash(&self.nest.len(), &mut h);
        for l in &self.nest {
            core::hash::Hash::hash(
                &(
                    l.mode,
                    l.pg,
                    l.ml,
                    l.prev_depth,
                    l.space_factor,
                    l.clang,
                    l.middle,
                    l.list.version().0,
                    &l.mlist,
                    &l.incompleat,
                    &l.lr_save,
                    &l.lr_box,
                ),
                &mut h,
            );
        }
        h.finish128()
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

    /// Level `p` of the nest (`nest_ptr` is `cur_list`).
    pub(crate) fn nest_at(&self, p: usize) -> &ListStateRecord {
        if p < self.nest.len() {
            self.nest_read();
        } else {
            // (the current level whole: its caller reads any field)
            for f in 0..crate::track::list::COUNT {
                self.list_read(f);
            }
        }
        self.nest.get(p).unwrap_or(&self.cur_list)
    }

    pub(crate) fn nest_at_mut(&mut self, p: usize) -> &mut ListStateRecord {
        if p < self.nest.len() {
            self.nest_read();
            self.nest_wrote();
        } else {
            for f in 0..crate::track::list::COUNT {
                self.list_read(f);
            }
            self.level_wrote();
        }
        match self.nest.get_mut(p) {
            Some(l) => l,
            None => &mut self.cur_list,
        }
    }

    /// The contribution list (§215: the outer level's list).
    pub(crate) fn contrib(&mut self) -> &mut partex_engine::nodelist::NodeList {
        if self.nest.is_empty() {
            return self.nodes_mut();
        }
        self.nest_read();
        self.nest_wrote();
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
        if nest_ptr > self.max_nest_stack {
            self.max_nest_stack = nest_ptr;
            if nest_ptr == usize::try_from(self.params.nest_size).unwrap_or(0) {
                let n = self.params.nest_size;
                return self.overflow(b"semantic nest size", n);
            }
        }
        // (the old level goes onto the nest whole: every field read)
        for f in 0..crate::track::list::COUNT {
            self.list_read(f);
        }
        self.nest_read();
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
        self.level_wrote();
    }
}
