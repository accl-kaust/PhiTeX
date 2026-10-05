//! cmap.c, cmap.h, cmap_p.h: CMaps (code to CID / code to Unicode) and
//! their cache.
//!
//! Conventions:
//! - A cached CMap is a `cmap_id: i32` into `self.cmap.cache` (C's
//!   `__cache`); `CMap_cache_get` / `CMap_cache_get_mut` borrow it. A CMap
//!   being built (`CMap_new`, not yet `CMap_cache_add`ed) is a local
//!   `CMap` passed as `&mut CMap` / `&CMap`.
//! - `useCMap` is the cache id of the used CMap (`CMap.use_cmap`), so the
//!   functions that follow it (`CMap_decode_char`, `CMap_decode`,
//!   `CMap_is_valid`, `CMap_set_usecmap`) are `Dpx` methods taking the
//!   CMap by reference (`&self` for the readers: pass
//!   `&self.cmap.cache[id]` via `CMap_cache_get`, or a local).
//! - The mapping tree: `map_tbl` is C's `mapTbl`, a table of 256 `MapDef`s
//!   whose `next` is the next table of 256. Codes are owned by their
//!   `MapDef` (C's `mapData` arena and `get_mem` are not needed).
//! - Decoding buffers: C's `(const unsigned char **inbuf, int *inbytesleft)`
//!   is `(inbuf: &[u8], inpos: &mut usize, inbytesleft: &mut i32)`, the
//!   pointer being `&inbuf[*inpos]` and the count kept separately as in C
//!   (`handle_undefined` advances the pointer without the count); the
//!   output likewise `(outbuf: &mut [u8], outpos, outbytesleft)`.
//! - `CMap_release` is `Drop`. Byte strings (`codeLo`, `src`…) are slices
//!   whose length is C's `dim` / `srcdim` / `destdim`.

#![allow(non_snake_case)]

use crate::pdffont::CidSysInfo;
use crate::prelude::*;

/// `CID`.
pub type Cid = u16;
/// `UCV16`.
pub type Ucv16 = u16;

/// `CID_MAX_CID`.
pub const CID_MAX_CID: i32 = 65535;
/// `CID_MAX`.
pub const CID_MAX: i32 = CID_MAX_CID;
/// `CID_NOTDEF_CHAR`.
pub const CID_NOTDEF_CHAR: &[u8; 2] = b"\0\0";
/// `CID_NOTDEF`.
pub const CID_NOTDEF: i32 = 0;
/// `UCS_NOTDEF_CHAR`.
pub const UCS_NOTDEF_CHAR: &[u8; 2] = b"\xff\xfd";
/// `UCS_NOTDEF`.
pub const UCS_NOTDEF: i32 = 0xfffd;

/// `CMAP_TYPE_IDENTITY`.
pub const CMAP_TYPE_IDENTITY: i32 = 0;
/// `CMAP_TYPE_CODE_TO_CID`.
pub const CMAP_TYPE_CODE_TO_CID: i32 = 1;
/// `CMAP_TYPE_TO_UNICODE`.
pub const CMAP_TYPE_TO_UNICODE: i32 = 2;
/// `CMAP_TYPE_CID_TO_CODE`.
pub const CMAP_TYPE_CID_TO_CODE: i32 = 3;

/// `CMAP_PROF_TYPE_INBYTES_MIN`.
pub const CMAP_PROF_TYPE_INBYTES_MIN: i32 = 0;
/// `CMAP_PROF_TYPE_INBYTES_MAX`.
pub const CMAP_PROF_TYPE_INBYTES_MAX: i32 = 1;
/// `CMAP_PROF_TYPE_OUTBYTES_MIN`.
pub const CMAP_PROF_TYPE_OUTBYTES_MIN: i32 = 2;
/// `CMAP_PROF_TYPE_OUTBYTES_MAX`.
pub const CMAP_PROF_TYPE_OUTBYTES_MAX: i32 = 3;

/// `MAP_IS_CID` (cmap_p.h).
pub const MAP_IS_CID: i32 = 1 << 0;
/// `MAP_IS_NAME`.
pub const MAP_IS_NAME: i32 = 1 << 1;
/// `MAP_IS_CODE`.
pub const MAP_IS_CODE: i32 = 1 << 2;
/// `MAP_IS_NOTDEF`.
pub const MAP_IS_NOTDEF: i32 = 1 << 3;
/// `MAP_IS_UNDEF`.
pub const MAP_IS_UNDEF: i32 = 0;
/// `MAP_TYPE_MASK`.
pub const MAP_TYPE_MASK: i32 = 0x00f;
/// `MAP_LOOKUP_END`.
pub const MAP_LOOKUP_END: i32 = 0;
/// `MAP_LOOKUP_CONTINUE`.
pub const MAP_LOOKUP_CONTINUE: i32 = 1 << 4;
/// `MEM_ALLOC_SIZE` (the C arena's segment size).
pub const MEM_ALLOC_SIZE: i32 = 4096;
/// `CMAP_CACHE_ALLOC_SIZE`.
pub const CMAP_CACHE_ALLOC_SIZE: u32 = 16;
/// `CMAP_DEBUG_STR`.
pub const CMAP_DEBUG_STR: &str = "CMap";

/// `MAP_DEFINED(e)`.
#[must_use]
pub fn MAP_DEFINED(e: i32) -> bool {
    (e & MAP_TYPE_MASK) != MAP_IS_UNDEF
}
/// `MAP_TYPE(e)`.
#[must_use]
pub fn MAP_TYPE(e: i32) -> i32 {
    e & MAP_TYPE_MASK
}
/// `LOOKUP_CONTINUE(f)`.
#[must_use]
pub fn LOOKUP_CONTINUE(f: i32) -> bool {
    f & MAP_LOOKUP_CONTINUE != 0
}
/// `LOOKUP_END(f)`.
#[must_use]
pub fn LOOKUP_END(f: i32) -> bool {
    !LOOKUP_CONTINUE(f)
}

/// cmap.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `__silent`.
    pub silent: i32,
    /// `__cache` (none until `CMap_cache_init`): ids 0 and 1 are
    /// Identity-H and Identity-V.
    pub cache: Option<Vec<CMap>>,
}

/// `rangeDef`: a codespace range (`dim` bytes each).
#[derive(Clone, Debug, Default)]
pub struct RangeDef {
    pub dim: i32,
    /// `codeLo`.
    pub code_lo: Vec<u8>,
    /// `codeHi`.
    pub code_hi: Vec<u8>,
}

/// `mapDef`: one entry of a 256-entry mapping table.
#[derive(Clone, Debug, Default)]
pub struct MapDef {
    pub flag: i32,
    /// 2 for a CID, variable for a code.
    pub len: i32,
    /// The CID (16-bit BE) or code (`len` bytes; empty for C's NULL).
    pub code: Vec<u8>,
    /// The next table (256 entries) for `MAP_LOOKUP_CONTINUE`.
    pub next: Option<Vec<MapDef>>,
}

/// `mapData`: C's storage arena for codes. Unused in the port (codes are
/// owned by `MapDef.code`); kept for reference.
#[derive(Clone, Debug, Default)]
pub struct MapData {
    pub data: Vec<u8>,
    pub pos: i32,
}

/// `CMap.profile`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CMapProfile {
    /// `minBytesIn`.
    pub min_bytes_in: i32,
    /// `maxBytesIn`.
    pub max_bytes_in: i32,
    /// `minBytesOut`.
    pub min_bytes_out: i32,
    /// `maxBytesOut`.
    pub max_bytes_out: i32,
}

/// `struct CMap`. `Default` is not `CMap_new` (use that for C's initial
/// values: the profile's minima are 65535, the type CODE_TO_CID).
#[derive(Clone, Debug, Default)]
pub struct CMap {
    pub name: Option<Vec<u8>>,
    /// `type`: CMapType.
    pub type_: i32,
    /// WMode: 0 horizontal, 1 vertical.
    pub wmode: i32,
    /// `CSI`.
    pub csi: Option<CidSysInfo>,
    /// `useCMap`: a cache id.
    pub use_cmap: Option<i32>,
    /// `codespace.ranges` (`num` is the length).
    pub codespace: Vec<RangeDef>,
    /// `mapTbl`: the first 256-entry table.
    pub map_tbl: Option<Vec<MapDef>>,
    /// Decoder flags (unused, as in C).
    pub flags: i32,
    pub profile: CMapProfile,
}

impl CMap {
    /// `CMap_new`.
    #[must_use]
    pub fn CMap_new() -> CMap {
        todo!()
    }
    /// `CMap_is_Identity`.
    #[must_use]
    pub fn CMap_is_Identity(&self) -> bool {
        todo!()
    }
    /// `CMap_get_profile`.
    #[must_use]
    pub fn CMap_get_profile(&self, type_: i32) -> i32 {
        todo!()
    }
    /// `CMap_get_name`.
    #[must_use]
    pub fn CMap_get_name(&self) -> Option<&[u8]> {
        todo!()
    }
    /// `CMap_get_type`.
    #[must_use]
    pub fn CMap_get_type(&self) -> i32 {
        todo!()
    }
    /// `CMap_get_wmode`.
    #[must_use]
    pub fn CMap_get_wmode(&self) -> i32 {
        todo!()
    }
    /// `CMap_get_CIDSysInfo`.
    #[must_use]
    pub fn CMap_get_CIDSysInfo(&self) -> Option<&CidSysInfo> {
        todo!()
    }
    /// `CMap_set_name`.
    pub fn CMap_set_name(&mut self, name: &[u8]) {
        todo!()
    }
    /// `CMap_set_type`.
    pub fn CMap_set_type(&mut self, type_: i32) {
        todo!()
    }
    /// `CMap_set_wmode`.
    pub fn CMap_set_wmode(&mut self, wmode: i32) {
        todo!()
    }
    /// `CMap_set_CIDSysInfo`: copies; none or an incomplete one clears it
    /// (with C's warning).
    pub fn CMap_set_CIDSysInfo(&mut self, csi: Option<&CidSysInfo>) {
        todo!()
    }
    /// `CMap_add_bfchar`.
    pub fn CMap_add_bfchar(&mut self, src: &[u8], dest: &[u8]) -> i32 {
        todo!()
    }
    /// `CMap_add_cidchar`.
    pub fn CMap_add_cidchar(&mut self, src: &[u8], dest: Cid) -> i32 {
        todo!()
    }
    /// `CMap_add_bfrange`: `srclo`, `srchi` of `srcdim` bytes.
    pub fn CMap_add_bfrange(&mut self, srclo: &[u8], srchi: &[u8], dest: &[u8]) -> i32 {
        todo!()
    }
    /// `CMap_add_cidrange`.
    pub fn CMap_add_cidrange(&mut self, srclo: &[u8], srchi: &[u8], base: Cid) -> i32 {
        todo!()
    }
    /// `CMap_add_notdefchar`.
    pub fn CMap_add_notdefchar(&mut self, src: &[u8], dst: Cid) -> i32 {
        todo!()
    }
    /// `CMap_add_notdefrange`.
    pub fn CMap_add_notdefrange(&mut self, srclo: &[u8], srchi: &[u8], dst: Cid) -> i32 {
        todo!()
    }
    /// `CMap_add_codespacerange`: `dim` is `codelo.len()`.
    pub fn CMap_add_codespacerange(&mut self, codelo: &[u8], codehi: &[u8]) -> i32 {
        todo!()
    }
    /// `CMap_match_codespace` (static): 0 if `c` is in a codespace range.
    fn CMap_match_codespace(&self, c: &[u8]) -> i32 {
        todo!()
    }
    /// `handle_undefined` (static): writes .notdef, advances `*inpos` by
    /// `bytes_consumed` (C does not decrease `*inbytesleft` here).
    fn handle_undefined(
        &self,
        inbuf: &[u8],
        inpos: &mut usize,
        inbytesleft: &mut i32,
        outbuf: &mut [u8],
        outpos: &mut usize,
        outbytesleft: &mut i32,
    ) {
        todo!()
    }
    /// `bytes_consumed` (static): `instr` is C's `(instr, inbytes)`.
    fn bytes_consumed(&self, instr: &[u8]) -> i32 {
        todo!()
    }
    /// `check_range` (static).
    fn check_range(&self, srclo: &[u8], srchi: &[u8], dst: &[u8]) -> i32 {
        todo!()
    }
}

/// `mapDef_new` (static): a table of 256 undefined entries.
fn mapDef_new() -> Vec<MapDef> {
    todo!()
}

/// `locate_tbl` (static): walks `cur` down the first `code.len() - 1`
/// bytes (creating tables), none for C's -1 ("Ambiguous CMap entry").
fn locate_tbl<'a>(cur: &'a mut Vec<MapDef>, code: &[u8]) -> Option<&'a mut Vec<MapDef>> {
    todo!()
}

impl Dpx {
    /// `CMap_set_silent`.
    pub fn CMap_set_silent(&mut self, value: i32) {
        todo!()
    }

    /// `CMap_is_valid`: follows `use_cmap` into the cache.
    pub fn CMap_is_valid(&self, cmap: &CMap) -> bool {
        todo!()
    }

    /// `CMap_set_usecmap`: `ucmap_id` is a cache id; `cmap` must not be
    /// borrowed from the cache (a CMap being parsed is a local).
    pub fn CMap_set_usecmap(&self, cmap: &mut CMap, ucmap_id: i32) {
        todo!()
    }

    /// `CMap_decode_char` (see the module doc for the buffers).
    pub fn CMap_decode_char(
        &self,
        cmap: &CMap,
        inbuf: &[u8],
        inpos: &mut usize,
        inbytesleft: &mut i32,
        outbuf: &mut [u8],
        outpos: &mut usize,
        outbytesleft: &mut i32,
    ) {
        todo!()
    }

    /// `CMap_decode`: the number of characters decoded.
    pub fn CMap_decode(
        &self,
        cmap: &CMap,
        inbuf: &[u8],
        inpos: &mut usize,
        inbytesleft: &mut i32,
        outbuf: &mut [u8],
        outpos: &mut usize,
        outbytesleft: &mut i32,
    ) -> i32 {
        todo!()
    }

    /// `CMap_cache_init`: Identity-H (0) and Identity-V (1).
    pub fn CMap_cache_init(&mut self) {
        todo!()
    }

    /// `CMap_cache_get`.
    pub fn CMap_cache_get(&self, id: i32) -> &CMap {
        todo!()
    }

    /// `CMap_cache_get`, to change a cached CMap (tt_cmap.c adds to one).
    pub fn CMap_cache_get_mut(&mut self, id: i32) -> &mut CMap {
        todo!()
    }

    /// `CMap_cache_find`: the id of a cached CMap by name, else loads it
    /// (`ResType::Cmap`); -1 if not found. The new id is reserved first
    /// (as C does), the CMap parsed as a local, then stored in its slot.
    pub fn CMap_cache_find(&mut self, cmap_name: &[u8]) -> i32 {
        todo!()
    }

    /// `CMap_cache_close`.
    pub fn CMap_cache_close(&mut self) {
        todo!()
    }

    /// `CMap_cache_add`: takes ownership; the new id.
    pub fn CMap_cache_add(&mut self, cmap: CMap) -> i32 {
        todo!()
    }
}
