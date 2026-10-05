//! cmap_read.c, cmap_read.h: reading a CMap file (PostScript resource).
//!
//! The file is read through C's `ifreader` buffer (`INPUT_BUF_SIZE`
//! chunks, refilled with `ifreader_read` before each token): tokens are
//! taken by `pst_get_token` from `buf[cursor..endptr]`, so where the
//! buffer ends matters as in C — keep its refill points exactly.

#![allow(non_snake_case)]

use crate::cmap::CMap;
use crate::prelude::*;

/// `CMAP_PARSE_DEBUG_STR`.
pub const CMAP_PARSE_DEBUG_STR: &str = "CMap_parse:";
/// `TOKEN_LEN_MAX`.
pub const TOKEN_LEN_MAX: usize = 127;
/// `INPUT_BUF_SIZE`.
pub const INPUT_BUF_SIZE: usize = 4096;
/// `CMAP_SIG_MAX`.
pub const CMAP_SIG_MAX: usize = 64;

/// `ifreader`: `buf[cursor..endptr]` is what is buffered (C keeps a NUL
/// at `endptr`; `buf` has room for it: `max + 1` bytes).
#[derive(Debug)]
pub struct Ifreader<'a> {
    pub cursor: usize,
    pub endptr: usize,
    pub buf: Vec<u8>,
    pub max: usize,
    pub fp: &'a mut MemFile,
    pub unread: usize,
}

/// `ifreader_create`.
fn ifreader_create(fp: &mut MemFile, size: usize, bufsize: usize) -> Ifreader<'_> {
    todo!()
}

/// `ifreader_read` (and `ifreader_need`): bytes available.
fn ifreader_read(reader: &mut Ifreader<'_>, size: usize) -> usize {
    todo!()
}

/// `check_next_token`: 0 if the next token is `key`, else -1.
fn check_next_token(input: &mut Ifreader<'_>, key: &[u8]) -> i32 {
    todo!()
}

/// `get_coderange`: status, and fills `code_lo`, `code_hi` (at most
/// `maxlen` bytes) and `*dim`.
fn get_coderange(
    input: &mut Ifreader<'_>,
    code_lo: &mut [u8],
    code_hi: &mut [u8],
    dim: &mut i32,
    maxlen: i32,
) -> i32 {
    todo!()
}

/// `do_codespacerange`.
fn do_codespacerange(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> i32 {
    todo!()
}

/// `handle_codearray`: `code_lo[..dim]`, its last byte incremented.
fn handle_codearray(
    cmap: &mut CMap,
    input: &mut Ifreader<'_>,
    code_lo: &mut [u8],
    dim: i32,
    count: i32,
) -> i32 {
    todo!()
}

/// `do_notdefrange`.
fn do_notdefrange(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> i32 {
    todo!()
}

/// `do_bfrange`.
fn do_bfrange(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> i32 {
    todo!()
}

/// `do_cidrange`.
fn do_cidrange(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> i32 {
    todo!()
}

/// `do_notdefchar`.
fn do_notdefchar(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> i32 {
    todo!()
}

/// `do_bfchar`.
fn do_bfchar(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> i32 {
    todo!()
}

/// `do_cidchar`.
fn do_cidchar(cmap: &mut CMap, input: &mut Ifreader<'_>, count: i32) -> i32 {
    todo!()
}

/// `do_cidsysteminfo`.
fn do_cidsysteminfo(cmap: &mut CMap, input: &mut Ifreader<'_>) -> i32 {
    todo!()
}

/// `CMap_parse_check_sig`: 0 for a CMap resource, else -1 (rewinds).
pub fn CMap_parse_check_sig(fp: &mut MemFile) -> i32 {
    todo!()
}

impl Dpx {
    /// `CMap_parse`: -1 on error, else `CMap_is_valid` (0 or 1). `cmap`
    /// is a local (a `usecmap` loads through the cache).
    pub fn CMap_parse(&mut self, cmap: &mut CMap, fp: &mut MemFile) -> i32 {
        todo!()
    }
}
