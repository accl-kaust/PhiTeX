//! cmap_write.c, cmap_write.h: a CMap as a PDF stream.
//!
//! C's `#if 0` parts (`CMap_ToCode_stream`, `add_inverse_map`, `add_map`,
//! `invert_cmap`, `flatten_cmap`) are not ported.

#![allow(non_snake_case)]

use crate::cmap::{CMap, MapDef};
use crate::prelude::*;

/// `BLOCK_LEN_MIN` (must be greater than 1).
pub const BLOCK_LEN_MIN: i32 = 2;
/// `WBUF_SIZE`.
pub const WBUF_SIZE: usize = 40960;

/// `CMAP_BEGIN`.
pub const CMAP_BEGIN: &[u8] = b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n";
/// `CMAP_END`.
pub const CMAP_END: &[u8] = b"endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";

/// `struct sbuf`: `buf` is `WBUF_SIZE` bytes, `curptr` and `limptr`
/// positions in it.
#[derive(Clone, Debug, Default)]
pub struct Sbuf {
    pub buf: Vec<u8>,
    pub curptr: usize,
    pub limptr: usize,
}

/// `block_count`: how many consecutive entries from `c` form a block.
fn block_count(mtab: &[MapDef], c: i32) -> i32 {
    todo!()
}

/// `sputx`: two uppercase hex digits at `buf[*s..]` (error past `end`).
fn sputx(c: u8, buf: &mut [u8], s: &mut usize, end: usize) -> i32 {
    todo!()
}

/// `write_string` (duplicated from pdfobj.c): a PDF literal string.
fn write_string(buf: &mut [u8], outptr: &mut usize, endptr: usize, string_data: &[u8]) {
    todo!()
}

/// `write_name`: a PDF name (`#xx` escapes).
fn write_name(buf: &mut [u8], outptr: &mut usize, endptr: usize, name_data: &[u8]) {
    todo!()
}

impl Dpx {
    /// `write_map`: `mtab` a 256-entry table, `codestr[..depth]` the code
    /// prefix; flushes `wbuf` into `stream` when full.
    fn write_map(
        &mut self,
        mtab: &[MapDef],
        count: i32,
        codestr: &mut [u8],
        depth: i32,
        wbuf: &mut Sbuf,
        stream: Obj,
    ) -> i32 {
        todo!()
    }

    /// `CMap_create_stream`: none for an invalid CMap. A `use_cmap` is
    /// referenced (`pdf_findresource`/`pdf_defineresource` "CMap") or
    /// created recursively. Pass a CMap not borrowed from `self` (clone
    /// it out of the cache if needed).
    pub fn CMap_create_stream(&mut self, cmap: &CMap) -> Option<Obj> {
        todo!()
    }
}
