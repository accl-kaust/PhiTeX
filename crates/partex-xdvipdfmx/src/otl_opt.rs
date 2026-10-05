//! otl_opt.c, otl_opt.h: OpenType layout option rules (`otl_tags`
//! expressions like `!latn&(kana|hira)`, matched against script tags).

use crate::prelude::*;

/// `OTL_OPTSTR_SEP`.
pub const OTL_OPTSTR_SEP: u8 = b'+';

/// `FLAG_NOT` (otl_opt.c).
pub const FLAG_NOT: i32 = 1 << 0;
/// `FLAG_AND` (otl_opt.c).
pub const FLAG_AND: i32 = 1 << 1;

/// `struct bt_node`: an expression tree node (a leaf holds a 4-byte tag
/// pattern, `?` matching any byte).
#[derive(Clone, Debug, Default)]
pub struct BtNode {
    pub flag: i32,
    pub left: Option<Box<BtNode>>,
    pub right: Option<Box<BtNode>>,
    pub data: [u8; 4],
}

/// `struct otl_opt` (`otl_opt`).
#[derive(Clone, Debug, Default)]
pub struct OtlOpt {
    pub rule: Option<Box<BtNode>>,
}

/// `match_expr` (static): 1 if `key` (4 bytes) matches.
fn match_expr(expr: Option<&BtNode>, key: &[u8]) -> i32 {
    todo!()
}

/// `bt_new_tree` (static).
fn bt_new_tree() -> Box<BtNode> {
    todo!()
}

/// `parse_expr` (static): `s` ends at C's `endptr`.
fn parse_expr(s: &[u8], pp: &mut usize) -> Option<Box<BtNode>> {
    todo!()
}

impl OtlOpt {
    /// `otl_new_opt`.
    #[must_use]
    pub fn otl_new_opt() -> OtlOpt {
        todo!()
    }
    /// `otl_release_opt`.
    pub fn otl_release_opt(self) {}
    /// `otl_parse_optstring`: 0.
    pub fn otl_parse_optstring(&mut self, optstr: Option<&[u8]>) -> i32 {
        todo!()
    }
}

/// `otl_match_optrule`: 1 if `tag` matches (always without a rule).
#[must_use]
pub fn otl_match_optrule(opt: Option<&OtlOpt>, tag: &[u8]) -> i32 {
    todo!()
}
