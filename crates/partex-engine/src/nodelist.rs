//! A node list as a value (DESIGN §7.17.12, the `cur_list` row): a
//! persistent sequence of nodes, `partex-ssa`'s [`PVec`], in which an
//! append shares the prefix and a copy is O(1), so a list is passed and
//! returned as a shared value. Each node is an immutable value versioned
//! when it is made ([`VNode`]); the list's version is made from its
//! nodes' by the sequence (a polynomial over the tree, the same whatever
//! the edits that made it).
//!
//! The current list, the nest's lists, and the page builder's lists (the
//! page and its discards) are this type. A box's list stays a vector:
//! packing reads it whole, and a box is a shared value of its own
//! (`BoxNode::share`).

use alloc::vec::Vec;
use core::fmt;
use core::ops::Index;

use partex_ssa::{PVec, Value, Version};

use crate::node::{Node, VERSIONS};

/// A node with the version it was made with (0 without [`VERSIONS`]).
#[derive(Clone)]
pub struct VNode {
    node: Node,
    ver: Version,
}

impl VNode {
    /// `node`, versioned now.
    #[must_use]
    pub fn new(node: Node) -> VNode {
        let ver = if VERSIONS.load(core::sync::atomic::Ordering::Relaxed) {
            // (a box by its own version: `Hash` of a versioned box is it)
            Version::of(&node)
        } else {
            Version::ABSENT
        };
        VNode { node, ver }
    }

    /// The node.
    #[must_use]
    pub fn node(&self) -> &Node {
        &self.node
    }

    /// The node, taken.
    #[must_use]
    pub fn into_node(self) -> Node {
        self.node
    }
}

impl Value for VNode {
    fn version(&self) -> Version {
        self.ver
    }
}

/// A persistent list of nodes.
#[derive(Clone, Default)]
pub struct NodeList(PVec<VNode>);

impl NodeList {
    #[must_use]
    pub fn new() -> NodeList {
        NodeList(PVec::new())
    }

    /// The list of `nodes`.
    #[must_use]
    pub fn from_vec(nodes: Vec<Node>) -> NodeList {
        NodeList(PVec::from_vec(nodes.into_iter().map(VNode::new).collect()))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The list's version, made from its nodes' (O(1)).
    #[must_use]
    pub fn version(&self) -> Version {
        self.0.version()
    }

    #[must_use]
    pub fn get(&self, i: usize) -> Option<&Node> {
        self.0.get(i).map(VNode::node)
    }

    #[must_use]
    pub fn first(&self) -> Option<&Node> {
        self.get(0)
    }

    #[must_use]
    pub fn last(&self) -> Option<&Node> {
        self.len().checked_sub(1).and_then(|i| self.get(i))
    }

    /// Append `n` (the prefix is shared).
    pub fn push(&mut self, n: Node) {
        self.0.push(VNode::new(n));
    }

    pub fn pop(&mut self) -> Option<Node> {
        let n = self.len().checked_sub(1)?;
        Some(self.0.remove(n).into_node())
    }

    /// Replace node `i` by `n`, a new value.
    pub fn set(&mut self, i: usize, n: Node) {
        self.0.set(i, VNode::new(n));
    }

    pub fn insert(&mut self, i: usize, n: Node) {
        self.0.insert(i, VNode::new(n));
    }

    pub fn remove(&mut self, i: usize) -> Node {
        self.0.remove(i).into_node()
    }

    /// Change the last node: it is made again (versioned) when `f` is
    /// done. `None` if the list is empty.
    pub fn edit_last<R>(&mut self, f: impl FnOnce(&mut Node) -> R) -> Option<R> {
        let i = self.len().checked_sub(1)?;
        Some(self.edit(i, f))
    }

    /// Change node `i`: it is made again when `f` is done.
    ///
    /// # Panics
    ///
    /// If `i` is past the end.
    pub fn edit<R>(&mut self, i: usize, f: impl FnOnce(&mut Node) -> R) -> R {
        let mut n = self.0.get(i).expect("a node").node.clone();
        let r = f(&mut n);
        self.set(i, n);
        r
    }

    /// §1034-like: append character `ch` of `font`, joining the glyph run
    /// at the end (`node::push_char`'s rule), the run made again.
    pub fn push_char(&mut self, font: crate::node::FontId, ch: u8) {
        if let Some(Node::Glyphs(g)) = self.last()
            && g.font == font
            && !g.is_full()
        {
            self.edit_last(|n| {
                if let Node::Glyphs(g) = n {
                    g.push(ch);
                }
            });
            return;
        }
        self.push(Node::Glyphs(crate::node::Glyphs::one(font, ch)));
    }

    /// [`NodeList::push_char`] of a character from `o`, its origin kept
    /// in `t` with its run's (`origin::push_char_org`'s rule).
    pub fn push_char_org(
        &mut self,
        font: crate::node::FontId,
        ch: u8,
        o: crate::origin::Org,
        t: &mut crate::origin::OrgTable,
    ) {
        if let Some(Node::Glyphs(g)) = self.last()
            && g.font == font
            && !g.is_full()
        {
            let h = t.run_push(g.org(), g.chars().len(), o);
            self.edit_last(|n| {
                if let Node::Glyphs(g) = n {
                    g.push(ch);
                    g.set_org(h);
                }
            });
            return;
        }
        let h = if o.is_none() { 0 } else { t.push(o) };
        self.push(Node::Glyphs(crate::node::Glyphs::one_at(font, ch, h)));
    }

    /// Keep the first `n` nodes.
    pub fn truncate(&mut self, n: usize) {
        if n == 0 {
            *self = NodeList::new();
            return;
        }
        while self.len() > n {
            self.0.remove(self.len() - 1);
        }
    }

    /// The nodes from `at` on, removed.
    #[must_use]
    pub fn split_off(&mut self, at: usize) -> NodeList {
        if at == 0 {
            return core::mem::take(self);
        }
        let tail: Vec<Node> = self.iter().skip(at).cloned().collect();
        self.truncate(at);
        NodeList::from_vec(tail)
    }

    /// Append the nodes of `other`, leaving it empty.
    pub fn append(&mut self, other: &mut NodeList) {
        let o = core::mem::take(other);
        if self.is_empty() {
            *self = o;
            return;
        }
        for v in &o.0 {
            self.0.push(v.clone());
        }
    }

    pub fn clear(&mut self) {
        *self = NodeList::new();
    }

    /// The nodes in order.
    pub fn iter(&self) -> impl Iterator<Item = &Node> + '_ {
        self.0.iter().map(VNode::node)
    }

    /// The nodes, copied into a vector (a box's list, a report).
    #[must_use]
    pub fn to_vec(&self) -> Vec<Node> {
        self.0.iter().map(|v| v.node.clone()).collect()
    }

    /// The nodes as a vector: moved out of the nodes no other list
    /// shares, copied from those it does.
    #[must_use]
    pub fn into_vec(self) -> Vec<Node> {
        self.0
            .into_vec()
            .into_iter()
            .map(VNode::into_node)
            .collect()
    }

    /// Whether the two lists share their tree's nodes (a copy of the same
    /// value).
    #[must_use]
    pub fn same(&self, other: &NodeList) -> bool {
        self.0.version() == other.0.version() && self.len() == other.len()
    }
}

impl Extend<Node> for NodeList {
    fn extend<I: IntoIterator<Item = Node>>(&mut self, it: I) {
        for n in it {
            self.push(n);
        }
    }
}

impl From<Vec<Node>> for NodeList {
    fn from(v: Vec<Node>) -> NodeList {
        NodeList::from_vec(v)
    }
}

impl FromIterator<Node> for NodeList {
    fn from_iter<I: IntoIterator<Item = Node>>(it: I) -> NodeList {
        NodeList::from_vec(it.into_iter().collect())
    }
}

impl Index<usize> for NodeList {
    type Output = Node;
    fn index(&self, i: usize) -> &Node {
        self.get(i).expect("a node of the list")
    }
}

impl PartialEq for NodeList {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl fmt::Debug for NodeList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl core::hash::Hash for NodeList {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        if VERSIONS.load(core::sync::atomic::Ordering::Relaxed) {
            // (a versioned list by its version: its nodes', once each)
            { self.version().0 }.hash(h);
            return;
        }
        self.len().hash(h);
        for n in self.iter() {
            n.hash(h);
        }
    }
}

impl crate::persist::Persist for NodeList {
    fn save(&self, s: &mut crate::persist::Saver) {
        self.to_vec().save(s);
    }
    fn load(l: &mut crate::persist::Loader) -> Option<Self> {
        let v: Vec<Node> = crate::persist::Persist::load(l)?;
        Some(NodeList::from_vec(v))
    }
}
