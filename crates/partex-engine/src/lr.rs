//! e-TeX's `TeXXeT`: mixed-direction text.
//!
//! `\beginL`, `\endL`, `\beginR`, `\endR` (and `\mathon`, `\mathoff`) are
//! math nodes whose subtypes nest like brackets. Each opening subtype `s`
//! is closed by [`end_of`]`(s)`; a list is balanced when every closing
//! node matches the innermost open one.

use alloc::vec::Vec;

use crate::node::Node;

/// `before`, `after`: `\mathon`, `\mathoff`.
pub const BEFORE: u8 = 0;
pub const AFTER: u8 = 1;
/// `L_code`, `R_code`: the text direction bits of a subtype.
pub const L_CODE: u8 = 4;
pub const R_CODE: u8 = 8;
/// Box subtypes (`box_lr`): an hlist already reversed, and a display
/// (never reversed).
pub const REVERSED: u8 = 1;
pub const DLIST: u8 = 2;
/// `begin_M_code`, `end_M_code`: the math nodes `TeXXeT` adds at line ends.
pub const BEGIN_M_CODE: u8 = 2;
pub const END_M_CODE: u8 = 3;

/// Does math subtype `s` close a segment (`end_LR`)?
#[must_use]
pub const fn is_end(s: u8) -> bool {
    s % 2 == 1
}

/// The subtype that closes segments of kind `s` (`end_LR_type`).
#[must_use]
pub const fn end_of(s: u8) -> u8 {
    L_CODE * (s / L_CODE) + END_M_CODE
}

/// The subtype that opens a segment `e` closes (`begin_LR_type`).
#[must_use]
pub const fn begin_of(e: u8) -> u8 {
    e - AFTER + BEFORE
}

/// Is segment kind `s` right-to-left (`LR_dir`)?
#[must_use]
pub const fn is_rtl(s: u8) -> bool {
    s / R_CODE == 1
}

/// What [`balance`] found wrong in an hlist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Problems {
    /// Closing nodes to append (innermost first): each open segment
    /// that was never closed.
    pub missing: Vec<Node>,
    /// Closing nodes that matched nothing (made into kerns).
    pub extra: u32,
}

impl Problems {
    #[must_use]
    pub fn any(&self) -> bool {
        !self.missing.is_empty() || self.extra > 0
    }
}

/// e-TeX's LR check of `hpack`: closing nodes that match no open segment
/// become explicit kerns of the same width; the closing nodes of segments
/// left open are returned to be appended.
pub fn balance(list: &mut [Node]) -> Problems {
    let mut open: Vec<u8> = Vec::new();
    let mut extra = 0;
    for n in list.iter_mut() {
        if let Node::Math {
            width,
            subtype,
            sync,
        } = *n
        {
            if !is_end(subtype) {
                open.push(end_of(subtype));
            } else if open.last() == Some(&end_of(subtype)) {
                open.pop();
            } else {
                extra += 1;
                // (the node made a kern, its place kept)
                *n = Node::Kern {
                    width,
                    subtype: 1, // `explicit`
                    sync,
                };
            }
        }
    }
    Problems {
        missing: open
            .into_iter()
            .rev()
            .map(|subtype| Node::Math {
                width: 0,
                subtype,
                sync: crate::origin::Side(0),
            })
            .collect(),
        extra,
    }
}
