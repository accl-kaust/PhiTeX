//! cff_dict.c, cff_dict.h: CFF DICT data.
//!
//! C's `stack_top`/`arg_stack` statics are reset at the start of each
//! `cff_dict_unpack` and only read below `stack_top`, so they are the
//! per-call [`DictStack`], not state. `cff_dict_update(cff->topdict, cff)`
//! in C: here take the dict out of the font first
//! (`let mut d = cff.topdict.take().unwrap(); cff_dict_update(&mut d, cff); cff.topdict = Some(d);`).

use crate::cff::*;
use crate::prelude::*;

pub const CFF_NOMINALWIDTHX_DEFAULT: f64 = 0.0;
pub const CFF_DEFAULTWIDTHX_DEFAULT: f64 = 0.0;

pub const CFF_PARSE_OK: i32 = 0;
pub const CFF_ERROR_PARSE_ERROR: i32 = -1;
pub const CFF_ERROR_STACK_OVERFLOW: i32 = -2;
pub const CFF_ERROR_STACK_UNDERFLOW: i32 = -3;
pub const CFF_ERROR_STACK_RANGECHECK: i32 = -4;

pub const DICT_ENTRY_MAX: i32 = 16;
pub const CFF_DICT_STACK_LIMIT: usize = 64;
pub const CFF_LAST_DICT_OP1: usize = 22;
pub const CFF_LAST_DICT_OP2: usize = 39;
pub const CFF_LAST_DICT_OP: usize = CFF_LAST_DICT_OP1 + CFF_LAST_DICT_OP2;

/// `dict_operator` (cff_dict.c): opname (none for the escape and reserved slots) and argument type.
pub static DICT_OPERATOR: [(Option<&[u8]>, i32); CFF_LAST_DICT_OP] = [
    (Some(b"version".as_slice()), CFF_TYPE_SID),
    (Some(b"Notice".as_slice()), CFF_TYPE_SID),
    (Some(b"FullName".as_slice()), CFF_TYPE_SID),
    (Some(b"FamilyName".as_slice()), CFF_TYPE_SID),
    (Some(b"Weight".as_slice()), CFF_TYPE_SID),
    (Some(b"FontBBox".as_slice()), CFF_TYPE_ARRAY),
    (Some(b"BlueValues".as_slice()), CFF_TYPE_DELTA),
    (Some(b"OtherBlues".as_slice()), CFF_TYPE_DELTA),
    (Some(b"FamilyBlues".as_slice()), CFF_TYPE_DELTA),
    (Some(b"FamilyOtherBlues".as_slice()), CFF_TYPE_DELTA),
    (Some(b"StdHW".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"StdVW".as_slice()), CFF_TYPE_NUMBER),
    (None, -1),
    (Some(b"UniqueID".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"XUID".as_slice()), CFF_TYPE_ARRAY),
    (Some(b"charset".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"Encoding".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"CharStrings".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"Private".as_slice()), CFF_TYPE_SZOFF),
    (Some(b"Subrs".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"defaultWidthX".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"nominalWidthX".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"Copyright".as_slice()), CFF_TYPE_SID),
    (Some(b"IsFixedPitch".as_slice()), CFF_TYPE_BOOLEAN),
    (Some(b"ItalicAngle".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"UnderlinePosition".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"UnderlineThickness".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"PaintType".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"CharstringType".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"FontMatrix".as_slice()), CFF_TYPE_ARRAY),
    (Some(b"StrokeWidth".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"BlueScale".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"BlueShift".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"BlueFuzz".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"StemSnapH".as_slice()), CFF_TYPE_DELTA),
    (Some(b"StemSnapV".as_slice()), CFF_TYPE_DELTA),
    (Some(b"ForceBold".as_slice()), CFF_TYPE_BOOLEAN),
    (None, -1),
    (None, -1),
    (Some(b"LanguageGroup".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"ExpansionFactor".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"InitialRandomSeed".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"SyntheticBase".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"PostScript".as_slice()), CFF_TYPE_SID),
    (Some(b"BaseFontName".as_slice()), CFF_TYPE_SID),
    (Some(b"BaseFontBlend".as_slice()), CFF_TYPE_DELTA),
    (None, -1),
    (None, -1),
    (None, -1),
    (None, -1),
    (None, -1),
    (None, -1),
    (Some(b"ROS".as_slice()), CFF_TYPE_ROS),
    (Some(b"CIDFontVersion".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"CIDFontRevision".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"CIDFontType".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"CIDCount".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"UIDBase".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"FDArray".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"FDSelect".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"FontName".as_slice()), CFF_TYPE_SID),
];

/// `stack_top`, `arg_stack` (C statics; per unpack here).
#[derive(Clone, Debug)]
pub struct DictStack {
    pub stack_top: i32,
    pub arg_stack: [f64; CFF_DICT_STACK_LIMIT],
}

impl Default for DictStack {
    fn default() -> Self {
        DictStack {
            stack_top: 0,
            arg_stack: [0.0; CFF_DICT_STACK_LIMIT],
        }
    }
}

/// `cff_new_dict`.
pub fn cff_new_dict() -> CffDict {
    todo!()
}

/// `cff_release_dict`.
pub fn cff_release_dict(dict: CffDict) {
    drop(dict);
}

/// `get_integer` (static): `*status` set on error.
fn get_integer(data: &[u8], p: &mut usize, status: &mut i32) -> f64 {
    todo!()
}

/// `get_real` (static).
fn get_real(data: &[u8], p: &mut usize, status: &mut i32) -> f64 {
    todo!()
}

/// `add_dict` (static): the operator at `data[*p]` with the stack's
/// operands.
fn add_dict(dict: &mut CffDict, st: &mut DictStack, data: &[u8], p: &mut usize, status: &mut i32) {
    todo!()
}

/// `cff_dict_unpack`: `data` ends at C's `endptr`; none on error.
pub fn cff_dict_unpack(data: &[u8]) -> Option<CffDict> {
    todo!()
}

/// `pack_integer` (static): bytes written.
fn pack_integer(dest: &mut [u8], value: i32) -> i32 {
    todo!()
}

/// `pack_real` (static): bytes written.
fn pack_real(dest: &mut [u8], value: f64) -> i32 {
    todo!()
}

/// `cff_dict_put_number` (static): bytes written.
fn cff_dict_put_number(value: f64, dest: &mut [u8], ty: i32) -> i32 {
    todo!()
}

/// `put_dict_entry` (static): bytes written.
fn put_dict_entry(de: &CffDictEntry, dest: &mut [u8]) -> i32 {
    todo!()
}

impl CffDict {
    /// `cff_dict_pack`: bytes written.
    pub fn cff_dict_pack(&self, dest: &mut [u8]) -> i32 {
        todo!()
    }
    /// `cff_dict_add`.
    pub fn cff_dict_add(&mut self, key: &[u8], count: i32) {
        todo!()
    }
    /// `cff_dict_remove`.
    pub fn cff_dict_remove(&mut self, key: &[u8]) {
        todo!()
    }
    /// `cff_dict_known`.
    pub fn cff_dict_known(&self, key: &[u8]) -> i32 {
        todo!()
    }
    /// `cff_dict_get`.
    pub fn cff_dict_get(&self, key: &[u8], idx: i32) -> f64 {
        todo!()
    }
    /// `cff_dict_set`.
    pub fn cff_dict_set(&mut self, key: &[u8], idx: i32, value: f64) {
        todo!()
    }
}

/// `cff_dict_update`: SIDs re-added to `cff`'s `_string` (see the
/// module doc for a dict of `cff` itself).
pub fn cff_dict_update(dict: &mut CffDict, cff: &mut CffFont) {
    todo!()
}
