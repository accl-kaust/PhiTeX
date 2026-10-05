//! pst.c, pst.h: a PostScript tokenizer (for Type 1 fonts).
//!
//! `pst_obj` is the enum [`PstObj`] (its functions, from pst_obj.c, are in
//! pst_obj.rs). Parsers `(unsigned char **inbuf, unsigned char *inbufend)`
//! are `(s: &[u8], p: &mut usize)` with `s` ending at `inbufend`; C
//! requires `*inbufend == 0`, so a read at `s.len()` is a 0 byte.

use crate::prelude::*;

/// `pst_type`.
pub type PstType = i32;

pub const PST_TYPE_UNKNOWN: PstType = -1;
pub const PST_TYPE_NULL: PstType = 0;
pub const PST_TYPE_BOOLEAN: PstType = 1;
pub const PST_TYPE_INTEGER: PstType = 2;
pub const PST_TYPE_REAL: PstType = 3;
pub const PST_TYPE_STRING: PstType = 5;
pub const PST_TYPE_NAME: PstType = 6;
pub const PST_TYPE_MARK: PstType = 7;

pub const PST_NAME_LEN_MAX: usize = 127;
pub const PST_STRING_LEN_MAX: usize = 32767;
pub const PST_MAX_DIGITS: usize = 10;
pub const PST_TOKEN_LEN_MAX: usize = PST_STRING_LEN_MAX;

/// `pst_obj`: its type and data.
#[derive(Clone, Debug, PartialEq)]
pub enum PstObj {
    /// `PST_TYPE_UNKNOWN`: an operator or other token, its text.
    Unknown(Vec<u8>),
    /// `PST_TYPE_NULL`.
    Null,
    /// `PST_TYPE_BOOLEAN` (C `char`).
    Boolean(i8),
    /// `PST_TYPE_INTEGER`.
    Integer(i32),
    /// `PST_TYPE_REAL`.
    Real(f64),
    /// `PST_TYPE_STRING`.
    String(Vec<u8>),
    /// `PST_TYPE_NAME` (without the `/`).
    Name(Vec<u8>),
    /// `PST_TYPE_MARK`.
    Mark,
}

impl Default for PstObj {
    fn default() -> Self {
        PstObj::Null
    }
}

impl PstObj {
    /// `PST_NULLTYPE`.
    pub fn is_null(&self) -> bool {
        todo!()
    }
    /// `PST_BOOLEANTYPE`.
    pub fn is_boolean(&self) -> bool {
        todo!()
    }
    /// `PST_NAMETYPE`.
    pub fn is_name(&self) -> bool {
        todo!()
    }
    /// `PST_STRINGTYPE`.
    pub fn is_string(&self) -> bool {
        todo!()
    }
    /// `PST_INTEGERTYPE`.
    pub fn is_integer(&self) -> bool {
        todo!()
    }
    /// `PST_REALTYPE`.
    pub fn is_real(&self) -> bool {
        todo!()
    }
    /// `PST_NUMBERTYPE`.
    pub fn is_number(&self) -> bool {
        todo!()
    }
    /// `PST_MARKTYPE`.
    pub fn is_mark(&self) -> bool {
        todo!()
    }
    /// `PST_UNKNOWNTYPE`.
    pub fn is_unknown(&self) -> bool {
        todo!()
    }
}

/// `PST_TOKEN_END(s, e)`: at the end, a delimiter or a space.
pub fn pst_token_end(s: &[u8], p: usize) -> bool {
    todo!()
}

/// `pst_parse_any` (static): a token up to the next delimiter, as
/// `Unknown`.
fn pst_parse_any(s: &[u8], p: &mut usize) -> Option<PstObj> {
    todo!()
}

/// `skip_line` (static).
fn skip_line(s: &[u8], p: &mut usize) {
    todo!()
}

/// `skip_comments` (static).
fn skip_comments(s: &[u8], p: &mut usize) {
    todo!()
}

/// `pst_get_token`: the next token, or none at the end.
pub fn pst_get_token(s: &[u8], p: &mut usize) -> Option<PstObj> {
    todo!()
}
