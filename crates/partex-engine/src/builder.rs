//! The page builder (tex.web part 45, §980–§1028).
//!
//! [`Builder`] is the current page and its measurements. The front end
//! moves contributions over after each thing it appends in vertical mode,
//! one [`Builder::step`] per node (a recorded call, DESIGN 7.17.2; the
//! caller keeps the contributions and hands each step its node). When a
//! page is complete, a step returns [`Step::FireUp`], and
//! [`Builder::fire_up`] cuts the page for box 255, fills the insertion
//! boxes and puts the rest back in front of the contributions. Whether
//! an output routine runs, or the page is shipped out, is up to the
//! caller. [`Builder::build`] is the steps over a list.
//!
//! Errors and `\tracingpages` lines come back as [`Event`]s, in order,
//! for the caller to print.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::Scaled;
use crate::node::{BoxNode, GlueSpec, Node, Order, Tokens};
use crate::pack::Confusion;
use crate::page::{self, precedes_break};
use crate::scaled::{INF_BAD, MAX_DIMEN, badness};

/// §157
const INF_PENALTY: i32 = 10000;
const EJECT_PENALTY: i32 = -10000;
/// §833, §974
const AWFUL_BAD: i32 = 0o7777777777;
const DEPLORABLE: i32 = 100_000;
/// §224: `top_skip_code`.
const TOP_SKIP: u8 = 9;

/// §980: `page_contents`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Contents {
    Empty,
    InsertsOnly,
    BoxThere,
}

crate::persist_enum!(Contents {
    Empty,
    InsertsOnly,
    BoxThere
});

/// §981: the state of one insertion class on the current page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InsState {
    /// All of it fits so far.
    Inserting,
    /// The insertion at page index `broken_ins` is split before index
    /// `broken_at` of its list (`None`: after all of it).
    SplitUp {
        broken_at: Option<usize>,
        broken_ins: usize,
    },
}

crate::persist_enum!(InsState { Inserting, SplitUp { broken_at, broken_ins } });

/// §981: a page insertion record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageIns {
    /// The class: box, `\count`, `\dimen` and `\skip` register `number`.
    pub number: u8,
    pub state: InsState,
    /// The natural height plus depth of box `number` and what goes into
    /// it on this page.
    pub height: Scaled,
    /// The page index of the last insertion of this class.
    pub last_ins: usize,
    /// `last_ins` at the best break so far (`None`: none go on the page).
    pub best_ins: Option<usize>,
}

crate::persist_struct!(PageIns {
    number,
    state,
    height,
    last_ins,
    best_ins
});

/// §982, §996: what `\lastskip`, `\lastpenalty`, `\lastkern` and
/// `\lastnodetype` see after the page builder took the last node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Last {
    pub glue: Option<GlueSpec>,
    pub penalty: i32,
    pub kern: Scaled,
    pub node_type: i32,
}

crate::persist_struct!(Last {
    glue,
    penalty,
    kern,
    node_type
});

impl Default for Last {
    fn default() -> Last {
        Last {
            glue: None,
            penalty: 0,
            kern: 0,
            node_type: -1,
        }
    }
}

/// §980–§982: the current page.
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Builder {
    pub contents: Contents,
    /// The page so far: a persistent list (`nodelist.rs`).
    pub list: crate::nodelist::NodeList,
    /// §982: `page_goal`, `page_total`, the stretch by order, the shrink
    /// and `page_depth`.
    pub so_far: [Scaled; 8],
    pub max_depth: Scaled,
    pub least_cost: i32,
    /// The page index of the best break so far; the length of the page
    /// is the node being contributed.
    pub best_break: Option<usize>,
    pub best_size: Scaled,
    /// Sorted by class.
    pub ins: Vec<PageIns>,
    pub insert_penalties: i32,
    pub last: Last,
    /// e-TeX: the items discarded at the top of the page, if saved
    /// (`page_disc`).
    pub discards: crate::nodelist::NodeList,
}

crate::persist_struct!(Builder {
    contents,
    list,
    so_far,
    max_depth,
    least_cost,
    best_break,
    best_size,
    ins,
    insert_penalties,
    last,
    discards
});

impl Default for Builder {
    fn default() -> Builder {
        Builder {
            contents: Contents::Empty,
            list: crate::nodelist::NodeList::new(),
            so_far: [0; 8],
            max_depth: 0,
            least_cost: 0,
            best_break: None,
            best_size: 0,
            ins: Vec::new(),
            insert_penalties: 0,
            last: Last::default(),
            discards: crate::nodelist::NodeList::new(),
        }
    }
}

/// Box register `n` as insertions see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InsBox {
    Void,
    Hbox,
    Vbox { height: Scaled, depth: Scaled },
}

/// What the page builder reads besides the page, each read where TeX
/// reads it: the parameters, and the registers of an insertion class.
pub trait Env {
    /// `\vsize` and `\maxdepth`, when the page's specs are frozen (§987).
    fn vsize(&self) -> Scaled;
    fn max_depth(&self) -> Scaled;
    /// `\topskip`, before the page's first box (§1001).
    fn top_skip(&self) -> GlueSpec;
    /// `\tracingpages>0` (§987, §1005, §1011).
    fn tracing(&self) -> bool;
    /// e-TeX's `\savingvdiscards>0` (§999): keep what is discarded at the
    /// top of the page (for `\pagediscards`).
    fn save_discards(&self) -> bool;
    /// Box, `\count`, `\dimen` and `\skip` register `n` (§1008–§1010).
    fn ins_box(&self, n: u8) -> InsBox;
    fn count(&self, n: u8) -> i32;
    fn dimen(&self, n: u8) -> Scaled;
    fn skip(&self, n: u8) -> GlueSpec;
}

/// §1000: what follows a kern on the contributions, the one thing a step
/// reads past its node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum After {
    /// The kern is the last contribution.
    Nothing,
    Glue,
    Other,
}

impl After {
    /// What `next` (the contribution after the kern, if any) is.
    #[must_use]
    pub fn of(next: Option<&Node>) -> After {
        match next {
            None => After::Nothing,
            Some(Node::Glue { .. } | Node::Leaders(_)) => After::Glue,
            Some(_) => After::Other,
        }
    }
}

/// The builder's fields, numbered as the core numbers its slots for them
/// (`track::page`), and two views of the list a step reads: what an
/// [`Access`] names.
pub mod field {
    /// `page_contents` (§980).
    pub const CONTENTS: u8 = 0;
    /// The page so far (§980).
    pub const LIST: u8 = 1;
    /// `page_so_far[k]` is `SO_FAR + k` (§982): the goal, the total, the
    /// stretch of each order, the shrink, the depth.
    pub const SO_FAR: u8 = 2;
    /// `page_max_depth` (§980).
    pub const MAX_DEPTH: u8 = 10;
    /// `least_page_cost`, `best_page_break`, `best_size` (§980).
    pub const LEAST_COST: u8 = 11;
    pub const BEST_BREAK: u8 = 12;
    pub const BEST_SIZE: u8 = 13;
    /// The page insertion records (§981).
    pub const INS: u8 = 14;
    /// `insert_penalties` (§982).
    pub const INSERT_PENALTIES: u8 = 15;
    /// `last_glue`, `last_penalty`, `last_kern`, `last_node_type` (§982,
    /// e-TeX).
    pub const LAST_GLUE: u8 = 16;
    pub const LAST_PENALTY: u8 = 17;
    pub const LAST_KERN: u8 = 18;
    pub const LAST_NODE_TYPE: u8 = 19;
    /// e-TeX's `page_disc` (`\pagediscards`).
    pub const DISCARDS: u8 = 20;
    /// The builder's fields end here.
    pub const COUNT: u8 = 21;
    /// What a step reads of the list: its length (the index its node
    /// takes, §1005, §1008) and whether its last node precedes a break
    /// (§1000). Views of `LIST`, never assigned by a step.
    pub const LIST_LEN: u8 = 21;
    pub const LIST_TAIL: u8 = 22;
}

/// What a step read and what it assigned of the page so far, by
/// [`field`], as TeX's step (§996–§1008) does: a field is read if the step
/// used its value before assigning it, and assigned if the step stored
/// into it, whether or not the value changed (DESIGN 7.17.2: a write of
/// an equal value is kept).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Access {
    read: u32,
    assigned: u32,
}

impl Access {
    fn bit(f: u8) -> u32 {
        1 << f
    }
    /// The step uses field `f`'s value.
    fn read(&mut self, f: u8) {
        if self.assigned & Self::bit(f) == 0 {
            self.read |= Self::bit(f);
        }
    }
    /// The step stores into field `f`.
    fn assign(&mut self, f: u8) {
        self.assigned |= Self::bit(f);
    }
    /// Both, in that order (`f := f + ...`).
    fn update(&mut self, f: u8) {
        self.read(f);
        self.assign(f);
    }
    /// The field of `page_so_far[k]`.
    fn so_far(k: usize) -> u8 {
        field::SO_FAR + u8::try_from(k & 7).unwrap_or(0)
    }
    /// The fields read, in order of number.
    pub fn reads(&self) -> impl Iterator<Item = u8> + '_ {
        (0..32).filter(|&f| self.read & Self::bit(f) != 0)
    }
    /// Whether field `f` was read.
    #[must_use]
    pub fn was_read(&self, f: u8) -> bool {
        self.read & Self::bit(f) != 0
    }
    /// The fields assigned, in order of number.
    pub fn assigned(&self) -> impl Iterator<Item = u8> + '_ {
        (0..32).filter(|&f| self.assigned & Self::bit(f) != 0)
    }
}

/// What one step of the page builder (§994–§1008) did with its node. A
/// step does not change the page's list or its discards: the caller
/// appends the node, so a step reads the list only by its length and
/// whether its last node precedes a break.
#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Step {
    /// §998: the node goes on the page (the caller links it in last).
    Page(Node),
    /// §999: the node is discarded; it is given back to keep for
    /// `\pagediscards` if `\savingvdiscards>0`.
    Discard(Option<Node>),
    /// §1001: the page's first box or rule. The `\topskip` glue goes in
    /// front of it on the contributions (glue first, then the node), and
    /// the next step takes the glue.
    TopSkip(Node, Node),
    /// §1000: a kern that is the last contribution stays there, and the
    /// builder returns: whether it is a breakpoint is not known yet.
    Kern(Node),
    /// §1005: the page is complete. The node stays the first
    /// contribution; call [`Builder::fire_up`].
    FireUp(Node),
}

/// What the caller prints (or does), in order.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// §987 tracing: `%% goal height=`.
    Freeze { goal: Scaled, max_depth: Scaled },
    /// §1006 tracing: `% t=... g= b= p= c=`, `#` if `best`; `AWFUL_BAD`
    /// prints as `*`.
    Cost {
        so_far: [Scaled; 8],
        b: i32,
        pi: i32,
        c: i32,
        best: bool,
    },
    /// §1004: "Infinite glue shrinkage found on current page".
    InfiniteShrinkPage,
    /// §993: box `n` is an hbox; the caller deletes it (`box_error`).
    NotVbox(u8),
    /// §1009: "Infinite glue shrinkage inserted from \skip n".
    InfiniteShrinkSkip(u8),
    /// §976: "Infinite glue shrinkage found in box being split".
    InfiniteShrinkSplit,
    /// §1011 tracing: `% split n to w,h p=pi`.
    Split {
        n: u8,
        w: Scaled,
        height_plus_depth: Scaled,
        pi: i32,
    },
}

/// Why [`Builder::build`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stop {
    /// The contributions are used up.
    Empty,
    /// A kern is the last contribution: whether it is a breakpoint is
    /// not known yet.
    Kern,
    /// The page is complete; the node that triggered it is the first
    /// contribution. Call [`Builder::fire_up`].
    FireUp,
}

/// §1012: what [`Builder::fire_up`] produces.
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Fired {
    pub events: Vec<Event>,
    /// `\outputpenalty`.
    pub output_penalty: i32,
    /// The page, for the caller to pack into box 255 (§1017:
    /// `vpackage(page, size, exactly, max_depth)` with `\vbadness` and
    /// `\vfuzz` inhibiting its report).
    pub page: Vec<Node>,
    pub size: Scaled,
    pub max_depth: Scaled,
    /// The new contents of the insertion boxes, by class.
    pub boxes: Vec<(u8, BoxNode)>,
}

/// §382: `top_mark`, `first_mark`, `bot_mark` (of one mark class).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Marks {
    pub top: Option<Tokens>,
    pub first: Option<Tokens>,
    pub bot: Option<Tokens>,
}

/// The page marks of every mark class (e-TeX's `\marks`; class 0 is
/// `\mark`).
pub type MarkClasses = BTreeMap<i32, Marks>;

/// §106: `x_over_n` (`n` is never 0 here).
fn x_over_n(x: Scaled, n: i32) -> Scaled {
    x / n
}

/// `\count n` scaling of an insertion height.
fn scaled_by_count(h: Scaled, count: i32) -> Scaled {
    if count == 1000 {
        h
    } else {
        x_over_n(h, 1000).wrapping_mul(count)
    }
}

/// tex.web's node type code, for `\lastnodetype`.
/// tex.web's node `type` of `n`.
#[must_use]
pub fn type_code(n: &Node) -> i32 {
    match n {
        Node::Glyphs(_) => 0,
        Node::Box(b) => i32::from(b.vertical),
        Node::Rule { .. } => 2,
        Node::Ins(_) => 3,
        Node::Mark(_) => 4,
        Node::Adjust(_) => 5,
        Node::Ligature(_) => 6,
        Node::Disc(_) => 7,
        Node::Whatsit(_) => 8,
        Node::Math { .. } => 9,
        Node::Glue { .. } | Node::Leaders(_) => 10,
        Node::Kern { .. } => 11,
        Node::Penalty(_) => 12,
        Node::Unset(_) => 13,
        Node::MarginKern { .. } => 40,
    }
}

enum Next {
    /// §999: discard the node.
    Discard,
    Break(i32),
    UpdateHeights,
    Contribute,
}

impl Builder {
    /// §991: start a new current page.
    pub fn start_new_page(&mut self) {
        self.contents = Contents::Empty;
        self.list.clear();
        self.last = Last::default();
        self.so_far[7] = 0;
        self.max_depth = 0;
    }

    /// Whether the page's last node precedes a break (§1000: then glue
    /// is a legal breakpoint).
    #[must_use]
    pub fn tail_precedes_break(&self) -> bool {
        self.list.last().is_some_and(precedes_break)
    }

    /// §987
    fn freeze(&mut self, s: Contents, env: &impl Env, events: &mut Vec<Event>, acc: &mut Access) {
        self.contents = s;
        self.so_far = [env.vsize(), 0, 0, 0, 0, 0, 0, 0];
        self.max_depth = env.max_depth();
        self.least_cost = AWFUL_BAD;
        acc.assign(field::CONTENTS);
        for k in 0..8 {
            let f = Access::so_far(k);
            acc.assign(f);
        }
        acc.assign(field::MAX_DEPTH);
        acc.assign(field::LEAST_COST);
        if env.tracing() {
            events.push(Event::Freeze {
                goal: self.so_far[0],
                max_depth: self.max_depth,
            });
        }
    }

    /// `page_so_far[k]`, read.
    fn sf(&self, k: usize, acc: &mut Access) -> Scaled {
        let f = Access::so_far(k);
        acc.read(f);
        self.so_far[k & 7]
    }

    /// `page_so_far[k] := v`.
    fn set_sf(&mut self, k: usize, v: Scaled, acc: &mut Access) {
        let f = Access::so_far(k);
        acc.assign(f);
        self.so_far[k & 7] = v;
    }

    /// §994: move nodes from `contrib` to the page until it is used up or
    /// the page is complete, one [`Builder::step`] per node.
    ///
    /// # Errors
    /// A confusion if a contribution cannot be on a vertical list.
    pub fn build(
        &mut self,
        contrib: &mut Vec<Node>,
        env: &impl Env,
        events: &mut Vec<Event>,
    ) -> Result<Stop, Confusion> {
        let mut rest: VecDeque<Node> = core::mem::take(contrib).into();
        let stop = loop {
            let Some(p) = rest.pop_front() else {
                break Ok(Stop::Empty);
            };
            let after = After::of(rest.front());
            match self.step(p, after, env, events, &mut Access::default()) {
                Ok(Step::Page(p)) => self.list.push(p),
                Ok(Step::Discard(p)) => self.discards.extend(p),
                Ok(Step::TopSkip(glue, p)) => {
                    rest.push_front(p);
                    rest.push_front(glue);
                }
                Ok(Step::Kern(p)) => {
                    rest.push_front(p);
                    break Ok(Stop::Kern);
                }
                Ok(Step::FireUp(p)) => {
                    rest.push_front(p);
                    break Ok(Stop::FireUp);
                }
                Err(c) => break Err(c),
            }
        };
        *contrib = rest.into();
        stop
    }

    /// §996–§1008: one step of `build_page` (§994): the first
    /// contribution `p` goes to the page, is discarded, or stays (the
    /// [`Step`] says which and gives it back). `after` is what follows
    /// `p` when it is a kern (§1000). `acc` gets the fields the step read
    /// and assigned.
    ///
    /// # Errors
    /// A confusion if `p` cannot be on a vertical list.
    pub fn step(
        &mut self,
        mut p: Node,
        after: After,
        env: &impl Env,
        events: &mut Vec<Event>,
        acc: &mut Access,
    ) -> Result<Step, Confusion> {
        use field::{CONTENTS, LIST_TAIL, MAX_DEPTH};
        // §996: update the values of `last_glue`, `last_penalty`, and
        // `last_kern`.
        self.last = Last {
            glue: match &p {
                Node::Glue { spec, .. } => Some(*spec),
                Node::Leaders(l) => Some(l.spec),
                _ => None,
            },
            penalty: if let Node::Penalty(pi) = p { pi } else { 0 },
            kern: if let Node::Kern { width, .. } = p {
                width
            } else {
                0
            },
            node_type: type_code(&p) + 1,
        };
        for f in field::LAST_GLUE..=field::LAST_NODE_TYPE {
            acc.assign(f);
        }
        // §997
        let next = match &p {
            // §1000
            Node::Box(_) | Node::Rule { .. } => {
                let (height, depth) = match &p {
                    Node::Box(b) => (b.height, b.depth),
                    Node::Rule { height, depth, .. } => (*height, *depth),
                    _ => unreachable!(),
                };
                acc.read(CONTENTS);
                if self.contents < Contents::BoxThere {
                    // §1001: initialize the current page, insert the
                    // \topskip glue ahead of `p`, and `goto continue`.
                    if self.contents == Contents::Empty {
                        self.freeze(Contents::BoxThere, env, events, acc);
                    } else {
                        self.contents = Contents::BoxThere;
                        acc.assign(CONTENTS);
                    }
                    let mut spec = env.top_skip().copy();
                    spec.width = if spec.width > height {
                        spec.width - height
                    } else {
                        0
                    };
                    let glue = Node::Glue {
                        spec,
                        subtype: TOP_SKIP + 1,
                        sync: crate::origin::Side(0),
                    };
                    return Ok(Step::TopSkip(glue, p));
                }
                // §1002
                let t = self.sf(1, acc) + self.sf(7, acc) + height;
                self.set_sf(1, t, acc);
                self.set_sf(7, depth, acc);
                Next::Contribute
            }
            Node::Glue { .. } | Node::Leaders(_) => {
                acc.read(CONTENTS);
                if self.contents < Contents::BoxThere {
                    Next::Discard
                } else {
                    acc.read(LIST_TAIL);
                    if self.tail_precedes_break() {
                        Next::Break(0)
                    } else {
                        Next::UpdateHeights
                    }
                }
            }
            Node::Kern { .. } => {
                acc.read(CONTENTS);
                if self.contents < Contents::BoxThere {
                    Next::Discard
                } else {
                    match after {
                        After::Nothing => return Ok(Step::Kern(p)),
                        After::Glue => Next::Break(0),
                        After::Other => Next::UpdateHeights,
                    }
                }
            }
            Node::Penalty(pi) => {
                acc.read(CONTENTS);
                if self.contents < Contents::BoxThere {
                    Next::Discard
                } else {
                    Next::Break(*pi)
                }
            }
            Node::Whatsit(w) => {
                // `XeTeX`: "Prepare to move whatsit |p| to the current page"
                if let crate::node::Whatsit::Pic(pic) = &**w {
                    let t = self.sf(1, acc) + self.sf(7, acc) + pic.height;
                    self.set_sf(1, t, acc);
                    self.set_sf(7, pic.depth, acc);
                }
                Next::Contribute
            }
            Node::Mark(_) => Next::Contribute,
            Node::Ins(_) => {
                self.append_insertion(&mut p, env, events, acc)?;
                Next::Contribute
            }
            _ => return Err(Confusion("page")),
        };
        let next = match next {
            Next::Break(pi) => {
                // §1005: check if node `p` is a new champion breakpoint;
                // then if it is time for a page break, stop.
                if pi < INF_PENALTY && self.champion(pi, env, events, acc) {
                    return Ok(Step::FireUp(p));
                }
                if matches!(p, Node::Penalty(_)) {
                    Next::Contribute
                } else {
                    Next::UpdateHeights
                }
            }
            next => next,
        };
        match next {
            Next::Discard => {
                // §999
                return Ok(Step::Discard(env.save_discards().then_some(p)));
            }
            Next::UpdateHeights => {
                // §1004
                let width = match &mut p {
                    Node::Kern { width, .. } => *width,
                    Node::Glue { spec, .. } => self.add_glue(spec, events, acc),
                    Node::Leaders(l) => self.add_glue(&mut l.spec, events, acc),
                    _ => unreachable!(),
                };
                let t = self.sf(1, acc) + self.sf(7, acc) + width;
                self.set_sf(1, t, acc);
                self.set_sf(7, 0, acc);
            }
            _ => {}
        }
        // contribute: §1003: make sure that `page_max_depth` is not
        // exceeded.
        acc.read(MAX_DEPTH);
        if self.sf(7, acc) > self.max_depth {
            let t = self.sf(1, acc) + self.so_far[7] - self.max_depth;
            self.set_sf(1, t, acc);
            self.set_sf(7, self.max_depth, acc);
        }
        // §998: link node `p` into the current page.
        Ok(Step::Page(p))
    }

    /// §1004: add glue `g` to the page totals, making infinite shrink
    /// finite; its width.
    fn add_glue(&mut self, g: &mut GlueSpec, events: &mut Vec<Event>, acc: &mut Access) -> Scaled {
        let k = 2 + g.stretch_order.index();
        let v = self.sf(k, acc) + g.stretch;
        self.set_sf(k, v, acc);
        let v = self.sf(6, acc) + g.shrink;
        self.set_sf(6, v, acc);
        if g.shrink_order != Order::Normal && g.shrink != 0 {
            events.push(Event::InfiniteShrinkPage);
            g.shrink_order = Order::Normal;
        }
        g.width
    }

    /// §1007: the badness of the page so far, `AWFUL_BAD` if too full.
    fn page_badness(&self, acc: &mut Access) -> i32 {
        let (total, goal) = (self.sf(1, acc), self.sf(0, acc));
        if total < goal {
            if self.sf(3, acc) != 0 || self.sf(4, acc) != 0 || self.sf(5, acc) != 0 {
                0
            } else {
                badness(goal - total, self.sf(2, acc))
            }
        } else if total - goal > self.sf(6, acc) {
            AWFUL_BAD
        } else {
            badness(total - goal, self.so_far[6])
        }
    }

    /// §1005–§1007: the node about to become page index `list.len()` is
    /// a breakpoint with penalty `pi`; `true` if it is time to fire up.
    fn champion(
        &mut self,
        pi: i32,
        env: &impl Env,
        events: &mut Vec<Event>,
        acc: &mut Access,
    ) -> bool {
        use field::{BEST_BREAK, BEST_SIZE, INS, INSERT_PENALTIES, LEAST_COST, LIST_LEN};
        let b = self.page_badness(acc);
        let mut c = if b < AWFUL_BAD {
            if pi <= EJECT_PENALTY {
                pi
            } else if b < INF_BAD {
                acc.read(INSERT_PENALTIES);
                b + pi + self.insert_penalties
            } else {
                DEPLORABLE
            }
        } else {
            b
        };
        acc.read(INSERT_PENALTIES);
        if self.insert_penalties >= 10000 {
            c = AWFUL_BAD;
        }
        acc.read(LEAST_COST);
        if env.tracing() {
            // (§1006 prints the goal and the totals)
            for k in 0..7 {
                self.sf(k, acc);
            }
            events.push(Event::Cost {
                so_far: self.so_far,
                b,
                pi,
                c,
                best: c <= self.least_cost,
            });
        }
        if c <= self.least_cost {
            acc.read(LIST_LEN);
            self.best_break = Some(self.list.len());
            acc.assign(BEST_BREAK);
            self.best_size = self.sf(0, acc);
            acc.assign(BEST_SIZE);
            self.least_cost = c;
            acc.assign(LEAST_COST);
            acc.read(INS);
            if !self.ins.is_empty() {
                acc.assign(INS);
            }
            for r in &mut self.ins {
                r.best_ins = Some(r.last_ins);
            }
        }
        c == AWFUL_BAD || pi <= EJECT_PENALTY
    }

    /// §1008: the insertion `p`, about to become page index `list.len()`.
    fn append_insertion(
        &mut self,
        p: &mut Node,
        env: &impl Env,
        events: &mut Vec<Event>,
        acc: &mut Access,
    ) -> Result<(), Confusion> {
        use field::{CONTENTS, INS, INSERT_PENALTIES, LIST_LEN};
        let Node::Ins(ins) = p else { unreachable!() };
        acc.read(CONTENTS);
        if self.contents == Contents::Empty {
            self.freeze(Contents::InsertsOnly, env, events, acc);
        }
        let n = ins.number;
        acc.read(INS);
        let k = match self.ins.binary_search_by_key(&n, |r| r.number) {
            Ok(k) => k,
            Err(k) => {
                // §1009: create a page insertion record, and include the
                // glue correction for box `n` in the current page state.
                let height = match env.ins_box(n) {
                    InsBox::Void => 0,
                    InsBox::Hbox => {
                        events.push(Event::NotVbox(n));
                        0
                    }
                    InsBox::Vbox { height, depth } => height + depth,
                };
                self.ins.insert(
                    k,
                    PageIns {
                        number: n,
                        state: InsState::Inserting,
                        height,
                        last_ins: 0,
                        best_ins: None,
                    },
                );
                acc.assign(INS);
                let q = env.skip(n);
                let h = scaled_by_count(height, env.count(n));
                let v = self.sf(0, acc) - (h + q.width);
                self.set_sf(0, v, acc);
                let o = 2 + q.stretch_order.index();
                let v = self.sf(o, acc) + q.stretch;
                self.set_sf(o, v, acc);
                let v = self.sf(6, acc) + q.shrink;
                self.set_sf(6, v, acc);
                if q.shrink_order != Order::Normal && q.shrink != 0 {
                    events.push(Event::InfiniteShrinkSkip(n));
                }
                k
            }
        };
        if let InsState::SplitUp { .. } = self.ins[k].state {
            acc.update(INSERT_PENALTIES);
            self.insert_penalties += ins.float_cost;
            return Ok(());
        }
        acc.read(LIST_LEN);
        let here = self.list.len();
        acc.assign(INS);
        self.ins[k].last_ins = here;
        // this much room is left if we shrink the maximum
        let delta = self.sf(0, acc) - self.sf(1, acc) - self.sf(7, acc) + self.sf(6, acc);
        let count = env.count(n);
        let h = scaled_by_count(ins.height, count); // this much room is needed
        let r = &mut self.ins[k];
        if (h <= 0 || h <= delta) && ins.height + r.height <= env.dimen(n) {
            r.height += ins.height;
            let v = self.so_far[0] - h;
            self.set_sf(0, v, acc);
            return Ok(());
        }
        // §1010: find the best way to split the insertion, and change its
        // state to `SplitUp`.
        let psf = self.so_far;
        let mut w = if count <= 0 {
            MAX_DIMEN
        } else {
            let mut w = psf[0] - psf[1] - psf[7];
            if count != 1000 {
                w = x_over_n(w, count).wrapping_mul(1000);
            }
            w
        };
        if w > env.dimen(n) - r.height {
            w = env.dimen(n) - r.height;
        }
        let brk = page::vert_break(&mut ins.list, w, ins.split_max_depth)?;
        for _ in 0..brk.infinite_shrink {
            events.push(Event::InfiniteShrinkSplit);
        }
        r.height += brk.best_height_plus_depth;
        let pi = match brk.at.map(|a| &ins.list[a]) {
            None => EJECT_PENALTY,
            Some(Node::Penalty(pi)) => *pi,
            Some(_) => 0,
        };
        if env.tracing() {
            // §1011
            events.push(Event::Split {
                n,
                w,
                height_plus_depth: brk.best_height_plus_depth,
                pi,
            });
        }
        let hpd = scaled_by_count(brk.best_height_plus_depth, count);
        r.state = InsState::SplitUp {
            broken_at: brk.at,
            broken_ins: here,
        };
        let v = self.so_far[0] - hpd;
        self.set_sf(0, v, acc);
        acc.update(INSERT_PENALTIES);
        self.insert_penalties += pi;
        Ok(())
    }

    /// §1012–§1022: cut the page at the best break for box 255, append
    /// the insertions to their boxes (`boxes` gives the current contents
    /// of a class's box; `vpack` is `vpack(list, natural)`, §1021, a
    /// routine of the caller's), update the marks, put the rest of the
    /// page before the contributions and start a new page holding the
    /// insertions held over. The caller has made sure box 255 is void
    /// (§1015), and packs the page (§1017).
    ///
    /// # Errors
    /// A confusion if the page's insertions do not match their records,
    /// or one from `vpack`.
    pub fn fire_up(
        &mut self,
        contrib: &mut Vec<Node>,
        holding_inserts: bool,
        marks: &mut MarkClasses,
        mut boxes: impl FnMut(u8) -> Option<BoxNode>,
        mut vpack: impl FnMut(Vec<Node>) -> Result<BoxNode, Confusion>,
    ) -> Result<Fired, Confusion> {
        let mut events = Vec::new();
        let best = self.best_break.unwrap_or(self.list.len());
        // §1013: set the value of `output_penalty`.
        let take = |n: &mut Node| {
            if let Node::Penalty(pi) = n {
                let v = *pi;
                *pi = INF_PENALTY;
                v
            } else {
                INF_PENALTY
            }
        };
        let output_penalty = if best < self.list.len() {
            // (the node made again: a list's nodes are values)
            self.list.edit(best, take)
        } else {
            take(&mut contrib[0])
        };
        for (&class, m) in marks.iter_mut() {
            if let Some(bot) = &m.bot {
                // (e-TeX drops an empty top mark of a class other than 0)
                m.top = (class == 0 || !bot.is_empty()).then(|| bot.clone());
                m.first = None;
                if m.top.is_none() {
                    m.bot = None;
                }
            }
        }
        self.insert_penalties = 0;
        // §1018: prepare all the boxes involved in insertions to act as
        // queues.
        let mut queues: Vec<Option<Vec<Node>>> = Vec::with_capacity(self.ins.len());
        for r in &self.ins {
            queues.push(if holding_inserts || r.best_ins.is_none() {
                None
            } else {
                Some(match boxes(r.number) {
                    Some(b) if !b.vertical => {
                        events.push(Event::NotVbox(r.number));
                        Vec::new()
                    }
                    Some(b) => b.list,
                    None => Vec::new(),
                })
            });
        }
        let mut out = Vec::new();
        let mut done = Vec::new();
        let mut held = Vec::new();
        let rest = self.list.split_off(best);
        for (i, p) in core::mem::take(&mut self.list)
            .into_vec()
            .into_iter()
            .enumerate()
        {
            match p {
                Node::Ins(mut ins) if !holding_inserts => {
                    // §1020: either insert the material specified by node
                    // `p` into the appropriate box, or hold it for the
                    // next page.
                    let k = self
                        .ins
                        .iter()
                        .position(|r| r.number == ins.number)
                        .ok_or(Confusion("fire_up"))?;
                    let r = &mut self.ins[k];
                    let mut wait = true;
                    if let Some(best_ins) = r.best_ins {
                        wait = false;
                        let q = queues[k].as_mut().ok_or(Confusion("fire_up"))?;
                        let mut list = core::mem::take(&mut ins.list);
                        if best_ins == i {
                            // §1021: wrap up the box, splitting `p` if
                            // called for.
                            if let InsState::SplitUp {
                                broken_at: Some(at),
                                broken_ins,
                            } = r.state
                                && broken_ins == i
                            {
                                let after = list.split_off(at);
                                let after = page::prune_page_top(after, ins.split_top, None)?;
                                if !after.is_empty() {
                                    let b = vpack(after.clone())?;
                                    ins.height = b.height + b.depth;
                                    wait = true;
                                }
                                ins.list = after;
                            }
                            r.best_ins = None;
                            q.extend(list);
                            let q = queues[k].take().unwrap_or_default();
                            done.push((r.number, vpack(q)?));
                        } else {
                            q.extend(list);
                        }
                    }
                    // §1022
                    if wait {
                        held.push(Node::Ins(ins));
                        self.insert_penalties += 1;
                    }
                }
                Node::Mark(m) => {
                    // §1016: update the values of `first_mark` and
                    // `bot_mark`.
                    let c = marks.entry(m.class).or_default();
                    if c.first.is_none() {
                        c.first = Some(m.tokens.clone());
                    }
                    c.bot = Some(m.tokens.clone());
                    out.push(Node::Mark(m));
                }
                p => out.push(p),
            }
        }
        // §1017: break the current page, put it in box 255, and put the
        // remaining nodes on the contribution list.
        if !rest.is_empty() {
            contrib.splice(0..0, rest.into_vec());
        }
        let (size, max_depth) = (self.best_size, self.max_depth);
        self.start_new_page();
        self.list = crate::nodelist::NodeList::from_vec(held);
        self.best_break = None;
        self.ins.clear();
        for m in marks.values_mut() {
            if m.top.is_some() && m.first.is_none() {
                m.first.clone_from(&m.top);
            }
        }
        done.sort_by_key(|(n, _)| *n);
        Ok(Fired {
            events,
            output_penalty,
            page: out,
            size,
            max_depth,
            boxes: done,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::GlueSign;
    use alloc::vec;

    /// The parameters, and no insertions.
    struct NoRegs {
        vsize: Scaled,
        max_depth: Scaled,
        top_skip: GlueSpec,
    }

    impl Env for NoRegs {
        fn vsize(&self) -> Scaled {
            self.vsize
        }
        fn max_depth(&self) -> Scaled {
            self.max_depth
        }
        fn top_skip(&self) -> GlueSpec {
            self.top_skip
        }
        fn tracing(&self) -> bool {
            false
        }
        fn save_discards(&self) -> bool {
            false
        }
        fn ins_box(&self, _: u8) -> InsBox {
            InsBox::Void
        }
        fn count(&self, _: u8) -> i32 {
            1000
        }
        fn dimen(&self, _: u8) -> Scaled {
            MAX_DIMEN
        }
        fn skip(&self, _: u8) -> GlueSpec {
            GlueSpec::default()
        }
    }

    fn line(height: Scaled, depth: Scaled) -> Node {
        Node::Box(
            (BoxNode {
                vertical: false,
                width: 0,
                height,
                depth,
                shift: 0,
                glue_set: 0.0,
                glue_sign: GlueSign::Normal,
                glue_order: Order::Normal,
                subtype: 0,
                list: Vec::new(),
                seal: None,
                ver: 0,
                sync: crate::origin::Side(0),
            })
            .share(),
        )
    }

    /// Three lines and `\penalty-10000`: the page is cut at the penalty,
    /// which stays on the contributions as `\penalty10000`.
    #[test]
    fn eject_fires_up() {
        let pt = 65536;
        let env = NoRegs {
            vsize: 50 * pt,
            max_depth: 2 * pt,
            top_skip: GlueSpec {
                width: 10 * pt,
                ..GlueSpec::default()
            },
        };
        let mut contrib = vec![
            line(7 * pt, 2 * pt),
            line(7 * pt, 2 * pt),
            line(7 * pt, 2 * pt),
            Node::Penalty(EJECT_PENALTY),
        ];
        let mut b = Builder::default();
        let mut events = Vec::new();
        let stop = b.build(&mut contrib, &env, &mut events).unwrap();
        assert_eq!(stop, Stop::FireUp);
        assert_eq!(b.list.len(), 4); // \topskip glue and three lines
        assert_eq!(b.so_far[1], 10 * pt + 2 * 9 * pt);
        let mut marks = MarkClasses::default();
        let fired = b
            .fire_up(
                &mut contrib,
                false,
                &mut marks,
                |_| None,
                |_| Err(Confusion("no insertions")),
            )
            .unwrap();
        assert_eq!(fired.output_penalty, EJECT_PENALTY);
        assert_eq!(fired.size, 50 * pt);
        assert_eq!(fired.page.len(), 4);
        assert_eq!(contrib, [Node::Penalty(INF_PENALTY)]);
        assert!(b.list.is_empty());
        assert_eq!(b.contents, Contents::Empty);
    }
}
