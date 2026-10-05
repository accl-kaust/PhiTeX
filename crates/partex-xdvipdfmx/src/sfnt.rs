//! sfnt.c, sfnt.h: TrueType/OpenType (sfnt) font files.
//!
//! An [`Sfnt`] owns its file (C's `FILE *stream`; the caller's `fp` is
//! moved in, `sfnt_close` is `Drop`). Every C function whose first
//! parameter is `sfnt *` and that touches no global is an `impl Sfnt`
//! method with the C name (also in tt_table.rs, tt_post.rs, tt_glyf.rs,
//! tt_cmap.rs, tt_aux.rs, tt_gsub.rs); one that makes PDF objects or reads
//! other global state is an `impl Dpx` method taking `sfont: &mut Sfnt`.
//! A CFF in an OpenType file is read from `sfont.stream.clone()` (shares
//! the bytes).

use crate::prelude::*;

pub type BYTE = u8;
pub type CHAR = i8;
pub type USHORT = u16;
pub type SHORT = i16;
pub type ULONG = u32;
pub type LONG = i32;
/// 16.16 fixed point (`uint32_t` in C).
pub type Fixed = u32;
pub type FWord = i16;
/// `uFWord`.
pub type UFWord = u16;

/// `SFNT_TABLE_REQUIRED`.
pub const SFNT_TABLE_REQUIRED: u8 = 1 << 0;

/// `SFNT_TYPE_TRUETYPE`.
pub const SFNT_TYPE_TRUETYPE: i32 = 1 << 0;
/// `SFNT_TYPE_OPENTYPE`.
pub const SFNT_TYPE_OPENTYPE: i32 = 1 << 1;
/// `SFNT_TYPE_POSTSCRIPT`.
pub const SFNT_TYPE_POSTSCRIPT: i32 = 1 << 2;
/// `SFNT_TYPE_TTC`.
pub const SFNT_TYPE_TTC: i32 = 1 << 4;
/// `SFNT_TYPE_DFONT`.
pub const SFNT_TYPE_DFONT: i32 = 1 << 8;

/// `SFNT_TRUETYPE` (sfnt.c): the version of a TrueType file.
pub const SFNT_TRUETYPE: u32 = 0x0001_0000;
/// `SFNT_MAC_TRUE` (`true`).
pub const SFNT_MAC_TRUE: u32 = 0x7472_7565;
/// `SFNT_OPENTYPE`.
pub const SFNT_OPENTYPE: u32 = 0x0001_0000;
/// `SFNT_POSTSCRIPT` (`OTTO`).
pub const SFNT_POSTSCRIPT: u32 = 0x4f54_544f;
/// `SFNT_TTC` (`ttcf`).
pub const SFNT_TTC: u32 = 0x7474_6366;

/// `struct sfnt_table`.
#[derive(Clone, Debug, Default)]
pub struct SfntTable {
    pub tag: [u8; 4],
    pub check_sum: ULONG,
    pub offset: ULONG,
    pub length: ULONG,
    /// The table's data when set by `sfnt_set_table` (C's `data`; none:
    /// read from the file at `offset`).
    pub data: Option<Vec<u8>>,
}

/// `struct sfnt_table_directory`.
#[derive(Clone, Debug, Default)]
pub struct SfntTableDirectory {
    /// Fixed for Win.
    pub version: ULONG,
    pub num_tables: USHORT,
    pub search_range: USHORT,
    pub entry_selector: USHORT,
    pub range_shift: USHORT,
    /// Number of kept tables.
    pub num_kept_tables: USHORT,
    /// Keep or omit (`SFNT_TABLE_REQUIRED`), one per table.
    pub flags: Vec<u8>,
    pub tables: Vec<SfntTable>,
}

/// `sfnt`.
#[derive(Clone, Debug)]
pub struct Sfnt {
    /// `SFNT_TYPE_*` (C's `type`).
    pub type_: i32,
    pub directory: Option<Box<SfntTableDirectory>>,
    pub stream: MemFile,
    pub offset: ULONG,
}

/// The `fixed(a)` macro: a 16.16 number as a double.
#[must_use]
pub fn fixed(a: Fixed) -> f64 {
    todo!()
}

/// `put_big_endian`: `q` as `n` bytes at the start of `s`; returns `n`.
pub fn put_big_endian(s: &mut [u8], q: LONG, n: i32) -> i32 {
    todo!()
}

/// `sfnt_put_ushort` macro.
pub fn sfnt_put_ushort(s: &mut [u8], v: USHORT) {
    todo!()
}

/// `sfnt_put_short` macro.
pub fn sfnt_put_short(s: &mut [u8], v: SHORT) {
    todo!()
}

/// `sfnt_put_ulong` macro.
pub fn sfnt_put_ulong(s: &mut [u8], v: ULONG) {
    todo!()
}

/// `sfnt_put_long` macro.
pub fn sfnt_put_long(s: &mut [u8], v: LONG) {
    todo!()
}

/// `convert_tag` (static): a 4-byte tag from a big-endian number.
fn convert_tag(u_tag: u32) -> [u8; 4] {
    todo!()
}

/// `max2floor` (static): the largest power of two `<= n`.
fn max2floor(n: u32) -> u32 {
    todo!()
}

/// `log2floor` (static).
fn log2floor(n: u32) -> u32 {
    todo!()
}

/// `sfnt_calc_checksum` (static).
fn sfnt_calc_checksum(data: &[u8]) -> ULONG {
    todo!()
}

/// `find_table_index` (static): the index of `tag`, or -1.
fn find_table_index(td: Option<&SfntTableDirectory>, tag: &[u8]) -> i32 {
    todo!()
}

impl Sfnt {
    /// `sfnt_open`: takes the file over; none if not an sfnt.
    pub fn sfnt_open(fp: MemFile) -> Option<Sfnt> {
        todo!()
    }
    /// `dfont_open`: the `index`th sfnt resource of a Mac dfont.
    pub fn dfont_open(fp: MemFile, index: i32) -> Option<Sfnt> {
        todo!()
    }
    /// `sfnt_close`.
    pub fn sfnt_close(self) {}

    /// `sfnt_get_byte`.
    pub fn sfnt_get_byte(&mut self) -> BYTE {
        todo!()
    }
    /// `sfnt_get_char`.
    pub fn sfnt_get_char(&mut self) -> CHAR {
        todo!()
    }
    /// `sfnt_get_ushort`.
    pub fn sfnt_get_ushort(&mut self) -> USHORT {
        todo!()
    }
    /// `sfnt_get_short`.
    pub fn sfnt_get_short(&mut self) -> SHORT {
        todo!()
    }
    /// `sfnt_get_ulong`.
    pub fn sfnt_get_ulong(&mut self) -> ULONG {
        todo!()
    }
    /// `sfnt_get_long`.
    pub fn sfnt_get_long(&mut self) -> LONG {
        todo!()
    }
    /// `sfnt_get_uint24`.
    pub fn sfnt_get_uint24(&mut self) -> ULONG {
        todo!()
    }
    /// `sfnt_seek_set`.
    pub fn sfnt_seek_set(&mut self, o: ULONG) {
        todo!()
    }
    /// `sfnt_read(b, l, s)`: fills `buf` (`l` = its length); bytes read.
    pub fn sfnt_read(&mut self, buf: &mut [u8]) -> usize {
        todo!()
    }

    /// `sfnt_read_table_directory`: 0, or -1 on error.
    pub fn sfnt_read_table_directory(&mut self, offset: ULONG) -> i32 {
        todo!()
    }
    /// `sfnt_find_table_len`: 0 if absent.
    pub fn sfnt_find_table_len(&self, tag: &[u8]) -> ULONG {
        todo!()
    }
    /// `sfnt_find_table_pos`: 0 if absent.
    pub fn sfnt_find_table_pos(&self, tag: &[u8]) -> ULONG {
        todo!()
    }
    /// `sfnt_locate_table`: seeks to the table and returns its offset
    /// (ERROR if absent).
    pub fn sfnt_locate_table(&mut self, tag: &[u8]) -> ULONG {
        todo!()
    }
    /// `sfnt_set_table`: `length` is `data.len()`.
    pub fn sfnt_set_table(&mut self, tag: &[u8], data: Vec<u8>) {
        todo!()
    }
    /// `sfnt_require_table`: 0, or -1 if a `must_exist` table is absent.
    pub fn sfnt_require_table(&mut self, tag: &[u8], must_exist: i32) -> i32 {
        todo!()
    }
}

impl Dpx {
    /// `sfnt_create_FontFile_stream`.
    pub fn sfnt_create_FontFile_stream(&mut self, sfont: &mut Sfnt) -> Option<Obj> {
        todo!()
    }
}
