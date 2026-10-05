//! pst_obj.c, pst_obj.h: [`PstObj`]'s functions and the parsers of each
//! type. Parsers are `(s: &[u8], p: &mut usize)` (see pst.rs).

use crate::prelude::*;
use crate::pst::{PstObj, PstType};

/// `pst_const_null`.
pub const PST_CONST_NULL: &[u8] = b"null";
/// `pst_const_mark`.
pub const PST_CONST_MARK: &[u8] = b"mark";

/// `pst_new_mark`.
pub fn pst_new_mark() -> PstObj {
    PstObj::Mark
}

/// `pst_release_obj`.
pub fn pst_release_obj(obj: PstObj) {
    drop(obj);
}

#[allow(non_snake_case)]
impl PstObj {
    /// `pst_type_of`.
    pub fn pst_type_of(&self) -> PstType {
        todo!()
    }
    /// `pst_length_of`.
    pub fn pst_length_of(&self) -> i32 {
        todo!()
    }
    /// `pst_getIV`.
    pub fn pst_getIV(&self) -> i32 {
        todo!()
    }
    /// `pst_getRV`.
    pub fn pst_getRV(&self) -> f64 {
        todo!()
    }
    /// `pst_getSV`: a new string (C: NUL-terminated copy), none for
    /// null and mark.
    pub fn pst_getSV(&self) -> Option<Vec<u8>> {
        todo!()
    }
    /// `pst_data_ptr`: the bytes of a name, string or unknown token
    /// (C points into the object; for numbers and booleans C points at
    /// the binary value: empty here; null and mark are a type error).
    pub fn pst_data_ptr(&self) -> &[u8] {
        todo!()
    }
}

/// `pst_parse_null`.
pub fn pst_parse_null(s: &[u8], p: &mut usize) -> Option<PstObj> {
    todo!()
}

/// `pst_parse_boolean`.
pub fn pst_parse_boolean(s: &[u8], p: &mut usize) -> Option<PstObj> {
    todo!()
}

/// `pst_parse_number`: integer, real, or radix (`16#ff`) number.
pub fn pst_parse_number(s: &[u8], p: &mut usize) -> Option<PstObj> {
    todo!()
}

/// `pst_name_is_valid` (static).
fn pst_name_is_valid(name: &[u8]) -> bool {
    todo!()
}

/// `putxpair` (static): `#xx` appended; the bytes written (3).
fn putxpair(c: u8, s: &mut Vec<u8>) -> i32 {
    todo!()
}

/// `pst_name_encode` (static).
fn pst_name_encode(name: &[u8]) -> Vec<u8> {
    todo!()
}

/// `getxpair` (static): the byte of two hex digits at `s[*p..]`, or -1.
fn getxpair(s: &[u8], p: &mut usize) -> i32 {
    todo!()
}

/// `pst_parse_name` (the `/` is required).
pub fn pst_parse_name(s: &[u8], p: &mut usize) -> Option<PstObj> {
    todo!()
}

/// `pst_string_parse_literal` (static).
fn pst_string_parse_literal(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    todo!()
}

/// `pst_string_parse_hex` (static).
fn pst_string_parse_hex(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    todo!()
}

/// `pst_parse_string`: literal `(…)` or hex `<…>`.
pub fn pst_parse_string(s: &[u8], p: &mut usize) -> Option<PstObj> {
    todo!()
}
