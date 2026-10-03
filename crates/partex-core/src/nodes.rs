//! Part 10: Data structures for boxes and their friends (§133–§161).
//!
//! Lists are the engine's typed nodes ([`partex_engine::node`]). tex.web's
//! node type and subtype codes remain: they are the `chr` codes of
//! commands like `\unskip` and the values TeX shows.

use partex_engine::Scaled;
use partex_engine::node::{GlueSpec, Node, RUNNING};

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;
#[allow(unused_imports)]
pub use crate::web::{
    A_LEADERS, ACC_KERN, ADJUST_NODE, AFTER, BEFORE, C_LEADERS, COND_MATH_GLUE, DEPTH_OFFSET,
    DISC_NODE, EJECT_PENALTY, EXPLICIT, FIL, FILL, FILLL, GLUE_NODE, HEIGHT_OFFSET, HLIST_NODE,
    INF_PENALTY, INS_NODE, KERN_NODE, LIGATURE_NODE, MARK_NODE, MATH_NODE, MU_GLUE, NORMAL,
    PENALTY_NODE, RULE_NODE, SHRINKING, STRETCHING, UNSET_NODE, VLIST_NODE, WHATSIT_NODE,
    WIDTH_OFFSET, X_LEADERS,
};

/// A node subtype code as the engine stores it (every code is below 256).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) const fn subtype(code: i32) -> u8 {
    code as u8
}

/// §139: a rule with running dimensions.
pub(crate) const NEW_RULE: Node = Node::Rule {
    width: RUNNING,
    height: RUNNING,
    depth: RUNNING,
    sync: partex_engine::origin::Side(0),
};

/// §153: glue with spec `spec`.
pub(crate) fn new_glue(spec: GlueSpec) -> Node {
    Node::Glue {
        spec,
        subtype: 0,
        sync: partex_engine::origin::Side(0),
    }
}

/// §156
pub(crate) fn new_kern(width: Scaled) -> Node {
    Node::Kern {
        width,
        subtype: 0,
        sync: partex_engine::origin::Side(0),
    }
}

/// §147
pub(crate) fn new_math(width: Scaled, s: i32) -> Node {
    Node::Math {
        width,
        subtype: u8::try_from(s).unwrap_or(0),
        sync: partex_engine::origin::Side(0),
    }
}

/// tex.web's `type` of a node (§133; §143 for characters, which are
/// `char_node`s, reported as -1 here).
pub(crate) fn node_type(n: &Node) -> i32 {
    match n {
        Node::Glyphs(_) => -1,
        Node::Box(b) if b.vertical => VLIST_NODE,
        Node::Box(_) => HLIST_NODE,
        Node::Rule { .. } => RULE_NODE,
        Node::Ins(_) => INS_NODE,
        Node::Mark(_) => MARK_NODE,
        Node::Adjust(_) => ADJUST_NODE,
        Node::Ligature(_) => LIGATURE_NODE,
        Node::Disc(_) => DISC_NODE,
        Node::Whatsit(_) => WHATSIT_NODE,
        Node::Math { .. } => MATH_NODE,
        Node::Glue { .. } | Node::Leaders(_) => GLUE_NODE,
        Node::Kern { .. } => KERN_NODE,
        Node::Penalty(_) => PENALTY_NODE,
        Node::Unset(_) => UNSET_NODE,
        Node::MarginKern { .. } => partex_engine::web::MARGIN_KERN_NODE,
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §152: glue for glue parameter `n` (sharing its spec).
    pub(crate) fn new_param_glue(&self, n: i32) -> Node {
        param_glue(self.glue_par(n), n)
    }
}

/// Glue for glue parameter `n` with spec `spec` (§152, §154).
pub(crate) fn param_glue(spec: GlueSpec, n: i32) -> Node {
    Node::Glue {
        spec,
        subtype: u8::try_from(n + 1).unwrap_or(0),
        sync: partex_engine::origin::Side(0),
    }
}
