//! Sealed lines (SSA mode, `DESIGN.md` 7.17.9's fold; the machine's,
//! old DESIGN §7.0).
//!
//! Once a paragraph is broken into lines, what is inside a line box (its
//! glue setting and list) is observed by very little: shipping the page
//! out, `\unhbox`, `\showbox` and the like, `\leftmarginkern`. Vertical
//! lists, the page builder and output routines see only a line's
//! dimensions. So an edit that changes the words of a line but not its
//! dimensions changes the page only inside its lines, which nothing reads
//! until the page is shipped.
//!
//! With sealing on, each line box appended by the line breaker keeps its
//! dimensions, and its contents move to a table under a key named by
//! where the paragraph was broken (the input file and line, how many
//! paragraphs were broken there before, the line's number): the same key
//! in two runs that broke the same paragraph, whatever its words. The
//! table is not part of `Rest` (the machine's coarse cell): each entry is
//! a cell of its own, written when the line is sealed and read where its
//! contents are looked at (`sealed_content`), so that only the region
//! that ships the page reads the words of an edited line.
//!
//! Entries are never removed (a copy of a line box, `\copy`, shares its
//! key); the table is sharded behind `Arc`s, so a snapshot shares it and
//! a write copies one shard.
//!
//! In SSA mode each entry is a slot (`track::Row::Sealed`), written by
//! the step that breaks the paragraph and read where the line is opened,
//! and its key is the step's id, the paragraph's count in the step and
//! the line's index: the same at each run of the step, and no other
//! line's. With the step boundary before `\shipout` (`CleanPoint::Ship`),
//! a word that leaves its line's dimensions runs again its paragraph's
//! step and the step that ships the page, not the output routine before
//! the `\shipout`.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::{Hash, Hasher as _};

use partex_engine::node::{BoxNode, GlueSign, Node, Order};
use partex_engine::stablehash::StableHasher;

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;

/// What a sealed line box holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Sealed {
    pub glue_set: f64,
    pub glue_sign: GlueSign,
    pub glue_order: Order,
    pub list: Vec<Node>,
    /// The hash of the rest (kept: a cell's version).
    version: u128,
}

impl Sealed {
    pub(crate) fn new(
        glue_set: f64,
        glue_sign: GlueSign,
        glue_order: Order,
        list: Vec<Node>,
    ) -> Self {
        let mut h = StableHasher::new();
        glue_set.to_bits().hash(&mut h);
        glue_sign.hash(&mut h);
        glue_order.hash(&mut h);
        list.hash(&mut h);
        Self {
            glue_set,
            glue_sign,
            glue_order,
            list,
            version: h.finish128(),
        }
    }

    /// The version of these contents (a cell's version).
    #[must_use]
    pub fn version(&self) -> u128 {
        self.version
    }
}

const SHARDS: usize = 64;

type Shard = BTreeMap<u128, Arc<Sealed>>;

/// The sealed lines, by key.
#[derive(Clone, Default)]
pub struct SealTable {
    shards: Vec<Arc<Shard>>,
}

impl core::fmt::Debug for SealTable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SealTable({})", self.len())
    }
}

impl SealTable {
    fn shard(k: u128) -> usize {
        usize::try_from(k % SHARDS as u128).unwrap_or(0)
    }

    #[must_use]
    pub fn get(&self, k: u128) -> Option<&Arc<Sealed>> {
        self.shards.get(Self::shard(k)).and_then(|s| s.get(&k))
    }

    pub fn insert(&mut self, k: u128, v: Arc<Sealed>) {
        if self.shards.is_empty() {
            self.shards = (0..SHARDS).map(|_| Arc::default()).collect();
        }
        Arc::make_mut(&mut self.shards[Self::shard(k)]).insert(k, v);
    }

    pub fn remove(&mut self, k: u128) {
        if let Some(s) = self.shards.get_mut(Self::shard(k))
            && s.contains_key(&k)
        {
            Arc::make_mut(s).remove(&k);
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.len()).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every entry's key and version, in key order within shards (for a
    /// whole-state digest).
    pub fn hash_into(&self, h: &mut StableHasher) {
        for s in &self.shards {
            for (k, v) in s.iter() {
                k.hash(h);
                h.write_u128(v.version());
            }
        }
    }
}

/// The slot of sealed line `k` (`track::Row::Sealed`): SSA mode's keys
/// are 64 bits.
#[allow(clippy::cast_possible_truncation, reason = "a key's low 64 bits")]
fn sealed_row(k: u128) -> u64 {
    k as u64
}

/// Does `n`, or anything inside it, hold a sealed line?
fn has_sealed(n: &Node) -> bool {
    match n {
        Node::Box(b) => b.seal.is_some() || b.list.iter().any(has_sealed),
        Node::Leaders(l) => has_sealed(&l.leader),
        Node::Ins(i) => i.list.iter().any(has_sealed),
        Node::Adjust(a) => a.list.iter().any(has_sealed),
        Node::Unset(u) => u.list.iter().any(has_sealed),
        _ => false,
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Turn sealing of lines on (a machine's engine) or off.
    pub fn set_seal_lines(&mut self, on: bool) {
        self.seal_lines = on;
    }

    /// The contents of the sealed line `k`, logged as read (a read of its
    /// slot, `track::Row::Sealed`, for a tracker that keeps versions).
    /// None only on a worker's view (`ssa/par.rs`), whose table holds the
    /// lines placed for its run: one it was not given taints the run,
    /// which is not taken ([`Tracker::seal_missing`]).
    pub(crate) fn sealed_content(&mut self, k: u128) -> Option<Arc<Sealed>> {
        let Some(s) = self.seals.get(k).cloned() else {
            assert!(self.tracker.seal_missing(), "a sealed line is in the table");
            return None;
        };
        if T::VALUES {
            self.tracker
                .value_read(crate::track::Row::Sealed(sealed_row(k)), || s.version());
        } else {
            self.seal_log.push((k, Some(s.version())));
        }
        Some(s)
    }

    /// A step begins (SSA mode): the paragraphs it breaks are counted
    /// from 0 ([`Tex::seal_paragraph`]), so each run of the step seals its
    /// lines under the same keys.
    pub(crate) fn seal_restart(&mut self) {
        self.seal_at = (0, 0);
    }

    /// Seal line `idx` of the paragraph just broken, `b`, if sealing is
    /// on: its contents go to the table, `b` keeps the rest.
    pub(crate) fn seal_line(&mut self, mut b: BoxNode, idx: u32) -> BoxNode {
        if !self.seal_lines || b.vertical {
            return b;
        }
        let mut h = StableHasher::new();
        (self.seal_at, idx).hash(&mut h);
        let mut k = h.finish128();
        if T::VALUES {
            // (SSA mode: the step's name and the paragraph's count in it
            // make the key no other line has, and the same at each run of
            // the step; 64 bits, a slot's address)
            k = u128::from(sealed_row(k));
        } else {
            // (the same place twice, a file read twice: the next free key,
            // which is the same in two runs that got here alike)
            while self.seals.get(k).is_some() {
                let mut h = StableHasher::new();
                k.hash(&mut h);
                k = h.finish128();
            }
        }
        let s = Sealed::new(
            core::mem::take(&mut b.glue_set),
            core::mem::replace(&mut b.glue_sign, GlueSign::Normal),
            core::mem::replace(&mut b.glue_order, Order::Normal),
            core::mem::take(&mut b.list),
        );
        self.seals.insert(k, Arc::new(s));
        if T::VALUES {
            self.tracker
                .value_wrote(crate::track::Row::Sealed(sealed_row(k)));
        } else {
            self.seal_log.push((k, None));
        }
        b.seal = Some(k);
        b
    }

    /// Where the paragraph being broken is, for its lines' keys: the input
    /// file and line, and how many paragraphs were broken there before.
    pub(crate) fn seal_paragraph(&mut self) {
        if !self.seal_lines {
            return;
        }
        let at = if T::VALUES {
            // (SSA mode: the step breaking it, from whose start paragraphs
            // are counted, `seal_restart`; never 0, the count's reset)
            u128::from(self.tracker.step_salt()) | 1 << 64
        } else {
            let mut h = StableHasher::new();
            let file = self
                .input_file
                .get(self.in_open)
                .and_then(Option::as_ref)
                .map(|f| f.name.clone());
            (file.as_deref(), self.in_open, self.line).hash(&mut h);
            h.finish128()
        };
        self.seal_at = if self.seal_at.0 == at {
            (at, self.seal_at.1 + 1)
        } else {
            (at, 0)
        };
    }

    /// Box `b` with its contents, if it is a sealed line (read).
    pub(crate) fn unsealed_box(&mut self, b: &BoxNode) -> Option<BoxNode> {
        let k = b.seal?;
        let s = self.sealed_content(k)?;
        let mut u = b.clone();
        u.seal = None;
        u.glue_set = s.glue_set;
        u.glue_sign = s.glue_sign;
        u.glue_order = s.glue_order;
        u.list.clone_from(&s.list);
        Some(u)
    }

    /// Box `b` with every sealed line inside it opened (read), if it
    /// holds any: what shipping it out sees.
    pub(crate) fn unsealed_deep(&mut self, b: &BoxNode) -> Option<BoxNode> {
        if b.seal.is_none() && !b.list.iter().any(has_sealed) {
            return None;
        }
        let mut u = self.unsealed_box(b).unwrap_or_else(|| b.clone());
        self.unseal_list(&mut u.list);
        Some(u)
    }

    fn unseal_list(&mut self, list: &mut [Node]) {
        for n in list.iter_mut() {
            if !has_sealed(n) {
                continue;
            }
            match n {
                Node::Box(b) => {
                    if let Some(u) = self.unsealed_deep(b) {
                        *b = Arc::new(u);
                    }
                }
                Node::Leaders(l) => {
                    let mut one = [core::mem::replace(&mut l.leader, Node::Penalty(0))];
                    self.unseal_list(&mut one);
                    let [x] = one;
                    l.leader = x;
                }
                Node::Ins(i) => self.unseal_list(&mut i.list),
                Node::Adjust(a) => self.unseal_list(&mut a.list),
                Node::Unset(u) => self.unseal_list(&mut u.list),
                _ => {}
            }
        }
    }
}

/// (A persisted machine's snapshots hold the table: `machine_store.rs`.)
impl partex_engine::persist::Persist for Sealed {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        let Self {
            glue_set,
            glue_sign,
            glue_order,
            list,
            version,
        } = self;
        glue_set.save(s);
        s.enc.u8(match glue_sign {
            GlueSign::Normal => 0,
            GlueSign::Stretching => 1,
            GlueSign::Shrinking => 2,
        });
        s.enc.u8(match glue_order {
            Order::Normal => 0,
            Order::Fil => 1,
            Order::Fill => 2,
            Order::Filll => 3,
        });
        list.save(s);
        version.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        use partex_engine::persist::Persist;
        let glue_set = f64::load(l)?;
        let glue_sign = match l.dec.u8()? {
            0 => GlueSign::Normal,
            1 => GlueSign::Stretching,
            2 => GlueSign::Shrinking,
            _ => return None,
        };
        let glue_order = match l.dec.u8()? {
            0 => Order::Normal,
            1 => Order::Fil,
            2 => Order::Fill,
            3 => Order::Filll,
            _ => return None,
        };
        Some(Self {
            glue_set,
            glue_sign,
            glue_order,
            list: Persist::load(l)?,
            version: Persist::load(l)?,
        })
    }
}

partex_engine::persist_struct!(SealTable { shards });
