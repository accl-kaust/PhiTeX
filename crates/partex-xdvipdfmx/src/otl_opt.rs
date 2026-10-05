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
    let mut retval = 1;
    if let Some(expr) = expr {
        if expr.left.is_none() && expr.right.is_none() {
            for i in 0..4 {
                if expr.data[i] != b'?' && expr.data[i] != key.get(i).copied().unwrap_or(0) {
                    retval = 0;
                    break;
                }
            }
        } else {
            if expr.left.is_some() {
                retval = match_expr(expr.left.as_deref(), key);
            }
            if expr.right.is_some() {
                if retval != 0 && (expr.flag & FLAG_AND) != 0 {
                    // and
                    retval &= match_expr(expr.right.as_deref(), key);
                } else if retval == 0 && (expr.flag & FLAG_AND) == 0 {
                    // or
                    retval = match_expr(expr.right.as_deref(), key);
                }
            }
        }
        if expr.flag & FLAG_NOT != 0 {
            // not
            retval = if retval != 0 { 0 } else { 1 };
        }
    }
    retval
}

/// `bt_new_tree` (static).
fn bt_new_tree() -> Box<BtNode> {
    Box::new(BtNode::default())
}

/// The node C's `curr` points to: the root until the first `|`/`&`, then
/// the root's right child (each `|`/`&` makes a new root).
fn curr_node(root: &mut BtNode, curr_is_root: bool) -> &mut BtNode {
    if curr_is_root {
        root
    } else {
        root.right.as_deref_mut().unwrap()
    }
}

/// `parse_expr` (static): `s` ends at C's `endptr`.
fn parse_expr(s: &[u8], pp: &mut usize) -> Option<Box<BtNode>> {
    let endptr = s.len();
    if *pp >= endptr {
        return None;
    }

    let mut root = bt_new_tree();
    let mut curr_is_root = true;
    while *pp < endptr {
        match s[*pp] {
            b'!' => {
                let curr = curr_node(&mut root, curr_is_root);
                if curr.flag & 2 != 0 {
                    curr.flag &= !FLAG_NOT;
                } else {
                    curr.flag |= FLAG_NOT;
                }
                *pp += 1;
            }
            b'(' => {
                *pp += 1;
                if *pp < endptr {
                    let Some(mut expr) = parse_expr(s, pp) else {
                        warn!("Syntax error: ...\n");
                        return None;
                    };
                    // C reads the NUL that ends the string at endptr.
                    if *pp >= endptr || s[*pp] != b')' {
                        warn!("Syntax error: Unbalanced ()\n");
                        return None;
                    }
                    let curr = curr_node(&mut root, curr_is_root);
                    curr.left = expr.left.take();
                    curr.right = expr.right.take();
                    curr.data = expr.data;
                } else {
                    warn!("Syntax error: Unbalanced ()\n");
                    return None;
                }
                *pp += 1;
            }
            b')' => return Some(root),
            c @ (b'|' | b'&') => {
                let mut tmp = bt_new_tree();
                tmp.left = Some(root);
                tmp.right = Some(bt_new_tree());
                curr_is_root = false;
                // C sets 1 (FLAG_NOT) for '&'.
                tmp.flag = if c == b'&' { 1 } else { 0 };
                root = tmp;
                *pp += 1;
            }
            b'*' => {
                let curr = curr_node(&mut root, curr_is_root);
                curr.data = [b'?'; 4];
                *pp += 1;
            }
            _ => {
                if *pp + 4 <= endptr {
                    for i in 0..4 {
                        let c = s[*pp];
                        let curr = curr_node(&mut root, curr_is_root);
                        if c == b' ' || c == b'?' || c.is_ascii_alphabetic() || c.is_ascii_digit() {
                            curr.data[i] = c;
                        } else if c == b'_' {
                            curr.data[i] = b' ';
                        } else {
                            warn!("Invalid char in tag: {}\n", c as char);
                            return None;
                        }
                        *pp += 1;
                    }
                } else {
                    warn!("Syntax error: ...\n");
                    return None;
                }
            }
        }
    }

    Some(root)
}

impl OtlOpt {
    /// `otl_new_opt`.
    #[must_use]
    pub fn otl_new_opt() -> OtlOpt {
        OtlOpt { rule: None }
    }
    /// `otl_release_opt`.
    pub fn otl_release_opt(self) {}
    /// `otl_parse_optstring`: 0.
    pub fn otl_parse_optstring(&mut self, optstr: Option<&[u8]>) -> i32 {
        if let Some(optstr) = optstr {
            let optstr = match optstr.iter().position(|&c| c == 0) {
                Some(n) => &optstr[..n],
                None => optstr,
            };
            let mut p = 0usize;
            self.rule = parse_expr(optstr, &mut p);
        }
        0
    }
}

/// `otl_match_optrule`: 1 if `tag` matches (always without a rule).
#[must_use]
pub fn otl_match_optrule(opt: Option<&OtlOpt>, tag: &[u8]) -> i32 {
    match opt {
        Some(OtlOpt { rule: Some(rule) }) => match_expr(Some(rule), tag),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(s: &[u8]) -> OtlOpt {
        let mut o = OtlOpt::otl_new_opt();
        o.otl_parse_optstring(Some(s));
        o
    }

    #[test]
    fn rules() {
        assert_eq!(otl_match_optrule(None, b"latn"), 1);
        let o = opt(b"*");
        assert_eq!(otl_match_optrule(Some(&o), b"latn"), 1);
        let o = opt(b"latn");
        assert_eq!(otl_match_optrule(Some(&o), b"latn"), 1);
        assert_eq!(otl_match_optrule(Some(&o), b"cyrl"), 0);
        let o = opt(b"kana|hira");
        assert_eq!(otl_match_optrule(Some(&o), b"hira"), 1);
        assert_eq!(otl_match_optrule(Some(&o), b"latn"), 0);
        let o = opt(b"!latn");
        assert_eq!(otl_match_optrule(Some(&o), b"latn"), 0);
        assert_eq!(otl_match_optrule(Some(&o), b"cyrl"), 1);
        // C's '&' sets FLAG_NOT (1), not FLAG_AND: "a&b" is !(a|b).
        let o = opt(b"latn&cyrl");
        assert_eq!(otl_match_optrule(Some(&o), b"latn"), 0);
        assert_eq!(otl_match_optrule(Some(&o), b"grek"), 1);
        let o = opt(b"(?lig|lig?|?cmp|cmp?|frac|afrc)");
        assert_eq!(otl_match_optrule(Some(&o), b"liga"), 1);
        assert_eq!(otl_match_optrule(Some(&o), b"ccmp"), 1);
        assert_eq!(otl_match_optrule(Some(&o), b"kern"), 0);
        let o = opt(b"ab_d");
        assert_eq!(otl_match_optrule(Some(&o), b"ab d"), 1);
    }
}
