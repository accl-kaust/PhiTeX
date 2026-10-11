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
    /// The code `src` maps to in this CMap's own table (a ToUnicode
    /// CMap's UTF-16BE), if it defines one (glyph runs; `use_cmap` not
    /// followed).
    #[must_use]
    pub fn lookup_code(&self, src: &[u8]) -> Option<&[u8]> {
        let mut t = self.map_tbl.as_ref()?;
        for (i, &c) in src.iter().enumerate() {
            let e = t.get(c as usize)?;
            if LOOKUP_END(e.flag) {
                return (i + 1 == src.len()
                    && MAP_DEFINED(e.flag)
                    && MAP_TYPE(e.flag) == MAP_IS_CODE)
                    .then(|| &e.code[..(e.len.max(0) as usize).min(e.code.len())]);
            }
            t = e.next.as_ref()?;
        }
        None
    }

    /// `CMap_new`.
    #[must_use]
    pub fn CMap_new() -> CMap {
        CMap {
            name: None,
            type_: CMAP_TYPE_CODE_TO_CID,
            wmode: 0,
            csi: None,
            use_cmap: None,
            codespace: Vec::with_capacity(10),
            map_tbl: None,
            flags: 0,
            profile: CMapProfile {
                min_bytes_in: 65535,
                max_bytes_in: 0,
                min_bytes_out: 65535,
                max_bytes_out: 0,
            },
        }
    }
    /// `CMap_is_Identity`.
    #[must_use]
    pub fn CMap_is_Identity(&self) -> bool {
        let name = self.name.as_deref().expect("CMap without a name");
        name == b"Identity-H" || name == b"Identity-V"
    }
    /// `CMap_get_profile` (C returns `maxBytesOut` for
    /// `CMAP_PROF_TYPE_OUTBYTES_MIN` too).
    #[must_use]
    pub fn CMap_get_profile(&self, type_: i32) -> Result<i32> {
        match type_ {
            CMAP_PROF_TYPE_INBYTES_MIN => Ok(self.profile.min_bytes_in),
            CMAP_PROF_TYPE_INBYTES_MAX => Ok(self.profile.max_bytes_in),
            CMAP_PROF_TYPE_OUTBYTES_MIN => Ok(self.profile.max_bytes_out),
            CMAP_PROF_TYPE_OUTBYTES_MAX => Ok(self.profile.max_bytes_out),
            _ => fatal!("{}: Unrecognized profile type {}.", CMAP_DEBUG_STR, type_),
        }
    }
    /// `CMap_get_name`.
    #[must_use]
    pub fn CMap_get_name(&self) -> Option<&[u8]> {
        self.name.as_deref()
    }
    /// `CMap_get_type`.
    #[must_use]
    pub fn CMap_get_type(&self) -> i32 {
        self.type_
    }
    /// `CMap_get_wmode`.
    #[must_use]
    pub fn CMap_get_wmode(&self) -> i32 {
        self.wmode
    }
    /// `CMap_get_CIDSysInfo`.
    #[must_use]
    pub fn CMap_get_CIDSysInfo(&self) -> Option<&CidSysInfo> {
        self.csi.as_ref()
    }
    /// `CMap_set_name` (C copies up to a NUL).
    pub fn CMap_set_name(&mut self, name: &[u8]) {
        self.name = Some(cstr(name).to_vec());
    }
    /// `CMap_set_type`.
    pub fn CMap_set_type(&mut self, type_: i32) {
        self.type_ = type_;
    }
    /// `CMap_set_wmode`.
    pub fn CMap_set_wmode(&mut self, wmode: i32) {
        self.wmode = wmode;
    }
    /// `CMap_set_CIDSysInfo`: copies; none or an incomplete one clears it
    /// (with C's warning).
    pub fn CMap_set_CIDSysInfo(&mut self, csi: Option<&CidSysInfo>) {
        self.csi = None;
        match csi {
            Some(csi) if csi.registry.is_some() && csi.ordering.is_some() => {
                self.csi = Some(CidSysInfo {
                    registry: csi.registry.as_deref().map(|s| cstr(s).to_vec()),
                    ordering: csi.ordering.as_deref().map(|s| cstr(s).to_vec()),
                    supplement: csi.supplement,
                });
            }
            _ => {
                warn!("Invalid CIDSystemInfo.");
                self.csi = None;
            }
        }
    }
    /// `CMap_add_bfchar`.
    pub fn CMap_add_bfchar(&mut self, src: &[u8], dest: &[u8]) -> i32 {
        self.CMap_add_bfrange(src, src, dest)
    }
    /// `CMap_add_cidchar`.
    pub fn CMap_add_cidchar(&mut self, src: &[u8], dest: Cid) -> i32 {
        self.CMap_add_cidrange(src, src, dest)
    }
    /// `CMap_add_bfrange`: `srclo`, `srchi` of `srcdim` (= `srclo.len()`)
    /// bytes, `base` of `dstdim` bytes.
    pub fn CMap_add_bfrange(&mut self, srclo: &[u8], srchi: &[u8], base: &[u8]) -> i32 {
        let srcdim = srclo.len();
        let dstdim = base.len();
        if self.check_range(srclo, srchi, base) < 0 {
            return -1;
        }
        let tbl = self.map_tbl.get_or_insert_with(mapDef_new);
        let Some(cur) = locate_tbl(tbl, srclo) else {
            return -1;
        };
        let lo = i32::from(srclo[srcdim - 1]);
        let hi = i32::from(srchi[srcdim - 1]);
        for c in lo..=hi {
            let e = &mut cur[c as usize];
            // Code mappings may overlap: later maps supersede earlier ones.
            if !MAP_DEFINED(e.flag) || e.len < dstdim as i32 {
                e.flag = MAP_LOOKUP_END | MAP_IS_CODE;
                e.code = vec![0; dstdim];
            }
            // (An entry kept keeps C's memory of at least `dstdim` bytes.)
            e.len = dstdim as i32;
            e.code[..dstdim].copy_from_slice(base);
            let mut last_byte = c - lo + i32::from(base[dstdim - 1]);
            e.code[dstdim - 1] = (last_byte & 0xFF) as u8;
            let mut i = dstdim as i32 - 2;
            while i >= 0 && last_byte > 255 {
                last_byte = i32::from(e.code[i as usize]) + 1;
                e.code[i as usize] = (last_byte & 0xFF) as u8;
                i -= 1;
            }
        }
        0
    }
    /// `CMap_add_cidrange`.
    pub fn CMap_add_cidrange(&mut self, srclo: &[u8], srchi: &[u8], base: Cid) -> i32 {
        let srcdim = srclo.len();
        let mut base = base;
        // base not used here (C passes its two bytes as `dst`)
        if self.check_range(srclo, srchi, &base.to_ne_bytes()) < 0 {
            return -1;
        }
        let tbl = self.map_tbl.get_or_insert_with(mapDef_new);
        let Some(cur) = locate_tbl(tbl, srclo) else {
            return -1;
        };
        let lo = i32::from(srclo[srcdim - 1]);
        let hi = i32::from(srchi[srcdim - 1]);
        for c in lo..=hi {
            let e = &mut cur[c as usize];
            if e.flag != 0 {
                warn!("Trying to redefine already defined CID mapping. (ignored)");
            } else {
                e.flag = MAP_LOOKUP_END | MAP_IS_CID;
                e.len = 2;
                e.code = vec![(base >> 8) as u8, (base & 0xff) as u8];
            }
            if i32::from(base) >= CID_MAX {
                warn!("CID number too large.");
            }
            base = base.wrapping_add(1);
        }
        0
    }
    /// `CMap_add_notdefchar`.
    pub fn CMap_add_notdefchar(&mut self, src: &[u8], dst: Cid) -> i32 {
        self.CMap_add_notdefrange(src, src, dst)
    }
    /// `CMap_add_notdefrange`.
    pub fn CMap_add_notdefrange(&mut self, srclo: &[u8], srchi: &[u8], dst: Cid) -> i32 {
        let srcdim = srclo.len();
        if self.check_range(srclo, srchi, &dst.to_ne_bytes()) < 0 {
            return -1;
        }
        let tbl = self.map_tbl.get_or_insert_with(mapDef_new);
        let Some(cur) = locate_tbl(tbl, srclo) else {
            return -1;
        };
        let lo = i32::from(srclo[srcdim - 1]);
        let hi = i32::from(srchi[srcdim - 1]);
        for c in lo..=hi {
            let e = &mut cur[c as usize];
            if MAP_DEFINED(e.flag) {
                warn!("Trying to redefine already defined code mapping. (ignored)");
            } else {
                e.flag = MAP_LOOKUP_END | MAP_IS_NOTDEF;
                e.code = vec![(dst >> 8) as u8, (dst & 0xff) as u8];
                e.len = 2;
            }
            // Do not do dst++ for notdefrange
        }
        0
    }
    /// `CMap_add_codespacerange`: `dim` is `codelo.len()`.
    pub fn CMap_add_codespacerange(&mut self, codelo: &[u8], codehi: &[u8]) -> i32 {
        let dim = codelo.len();
        assert!(dim > 0);
        for csr in &self.codespace {
            let mut overlap = true;
            let mut j = 0;
            while j < (csr.dim as usize).min(dim) && overlap {
                overlap = (codelo[j] >= csr.code_lo[j] && codelo[j] <= csr.code_hi[j])
                    || (codehi[j] >= csr.code_lo[j] && codehi[j] <= csr.code_hi[j]);
                j += 1;
            }
            if overlap {
                warn!("Overlapping codespace found. (ingored)");
                return -1;
            }
        }
        let dim = dim as i32;
        if dim < self.profile.min_bytes_in {
            self.profile.min_bytes_in = dim;
        }
        if dim > self.profile.max_bytes_in {
            self.profile.max_bytes_in = dim;
        }
        self.codespace.push(RangeDef {
            dim,
            code_lo: codelo.to_vec(),
            code_hi: codehi[..dim as usize].to_vec(),
        });
        0
    }
    /// `CMap_match_codespace` (static): 0 if `c` is in a codespace range.
    fn CMap_match_codespace(&self, c: &[u8]) -> i32 {
        let dim = c.len() as i32;
        for csr in &self.codespace {
            if csr.dim != dim {
                continue;
            }
            let mut pos = 0;
            while pos < dim as usize {
                if c[pos] > csr.code_hi[pos] || c[pos] < csr.code_lo[pos] {
                    break;
                }
                pos += 1;
            }
            if pos == dim as usize {
                return 0; // Valid
            }
        }
        -1 // Invalid
    }
    /// `handle_undefined` (static): writes .notdef, advances `*inpos` and
    /// decreases `*inbytesleft` by `bytes_consumed`.
    fn handle_undefined(
        &self,
        inbuf: &[u8],
        inpos: &mut usize,
        inbytesleft: &mut i32,
        outbuf: &mut [u8],
        outpos: &mut usize,
        outbytesleft: &mut i32,
    ) -> Result<()> {
        if *outbytesleft < 2 {
            fatal!("{}: Buffer overflow.", CMAP_DEBUG_STR);
        }
        let o = *outpos;
        match self.type_ {
            CMAP_TYPE_CODE_TO_CID => outbuf[o..o + 2].copy_from_slice(CID_NOTDEF_CHAR),
            CMAP_TYPE_TO_UNICODE => outbuf[o..o + 2].copy_from_slice(UCS_NOTDEF_CHAR),
            _ => {
                warn!(
                    "Cannot handle undefined mapping for this type of CMap mapping: {}",
                    self.type_
                );
                warn!("<0000> is used for .notdef char.");
                outbuf[o..o + 2].fill(0);
            }
        }
        *outpos += 2;
        *outbytesleft -= 2;

        let n = (*inbytesleft).max(0) as usize;
        let end = (*inpos + n).min(inbuf.len());
        let len = self.bytes_consumed(&inbuf[*inpos..end]);
        *inpos = (*inpos as isize + len as isize) as usize;
        *inbytesleft -= len;
        Ok(())
    }
    /// `bytes_consumed` (static): `instr` is C's `(instr, inbytes)`. (C's
    /// outer loop never breaks, so it is always `minBytesIn` unless a
    /// range matches in full.)
    fn bytes_consumed(&self, instr: &[u8]) -> i32 {
        let inbytes = instr.len();
        let mut longest = 0usize;
        let mut i = 0;
        while i < self.codespace.len() {
            let csr = &self.codespace[i];
            let mut pos = 0;
            while pos < (csr.dim as usize).min(inbytes) {
                if instr[pos] > csr.code_hi[pos] || instr[pos] < csr.code_lo[pos] {
                    break;
                }
                pos += 1;
            }
            if pos == csr.dim as usize {
                // part of instr is totally valid in this codespace.
                return csr.dim;
            }
            if pos > longest {
                longest = pos;
            }
            i += 1;
        }
        let mut bytesconsumed;
        if i == self.codespace.len() {
            // No matching at all
            bytesconsumed = self.profile.min_bytes_in;
        } else {
            bytesconsumed = self.profile.max_bytes_in;
            for csr in &self.codespace {
                if csr.dim > longest as i32 && csr.dim < bytesconsumed {
                    bytesconsumed = csr.dim;
                }
            }
        }
        bytesconsumed
    }
    /// `check_range` (static): `dst` of `dstdim` bytes. Updates the
    /// profile.
    fn check_range(&mut self, srclo: &[u8], srchi: &[u8], dst: &[u8]) -> i32 {
        let srcdim = srclo.len();
        let dstdim = dst.len();
        if srcdim < 1
            || dstdim < 1
            || srchi.len() < srcdim
            || srclo[..srcdim - 1] != srchi[..srcdim - 1]
            || srclo[srcdim - 1] > srchi[srcdim - 1]
        {
            warn!("Invalid CMap mapping entry. (ignored)");
            return -1;
        }
        if self.CMap_match_codespace(srclo) < 0 || self.CMap_match_codespace(&srchi[..srcdim]) < 0 {
            warn!("Invalid CMap mapping entry. (ignored)");
            return -1;
        }
        let (srcdim, dstdim) = (srcdim as i32, dstdim as i32);
        if srcdim < self.profile.min_bytes_in {
            self.profile.min_bytes_in = srcdim;
        }
        if srcdim > self.profile.max_bytes_in {
            self.profile.max_bytes_in = srcdim;
        }
        if dstdim < self.profile.min_bytes_out {
            self.profile.min_bytes_out = dstdim;
        }
        if dstdim > self.profile.max_bytes_out {
            self.profile.max_bytes_out = dstdim;
        }
        0
    }
}

/// A C string: the bytes before the first NUL.
fn cstr(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `mapDef_new` (static): a table of 256 undefined entries.
fn mapDef_new() -> Vec<MapDef> {
    let mut t = Vec::with_capacity(256);
    t.resize_with(256, || MapDef {
        flag: MAP_LOOKUP_END | MAP_IS_UNDEF,
        len: 0,
        code: Vec::new(),
        next: None,
    });
    t
}

/// `locate_tbl` (static): walks `cur` down the first `code.len() - 1`
/// bytes (creating tables), none for C's -1 ("Ambiguous CMap entry").
fn locate_tbl<'a>(cur: &'a mut Vec<MapDef>, code: &[u8]) -> Option<&'a mut Vec<MapDef>> {
    let mut cur = cur;
    let dim = code.len();
    for i in 0..dim.saturating_sub(1) {
        let c = code[i] as usize;
        if MAP_DEFINED(cur[c].flag) {
            warn!("Ambiguous CMap entry.");
            return None;
        }
        let e = &mut cur[c];
        if e.next.is_none() {
            // create new node
            e.next = Some(mapDef_new());
        }
        e.flag |= MAP_LOOKUP_CONTINUE;
        cur = e.next.as_mut().unwrap();
    }
    Some(cur)
}

impl Dpx {
    /// `CMap_set_silent`.
    pub fn CMap_set_silent(&mut self, value: i32) {
        self.cmap.silent = if value != 0 { 1 } else { 0 };
    }

    /// `CMap_is_valid`: follows `use_cmap` into the cache.
    pub fn CMap_is_valid(&self, cmap: &CMap) -> Result<bool> {
        // Quick check
        if cmap.name.is_none()
            || cmap.type_ < CMAP_TYPE_IDENTITY
            || cmap.type_ > CMAP_TYPE_CID_TO_CODE
            || cmap.codespace.is_empty()
            || (cmap.type_ != CMAP_TYPE_IDENTITY && cmap.map_tbl.is_none())
        {
            return Ok(false);
        }
        if let Some(id) = cmap.use_cmap {
            let ucmap = self.CMap_cache_get(id)?;
            let csi1 = cmap
                .CMap_get_CIDSysInfo()
                .expect("CMap without CIDSystemInfo");
            let csi2 = ucmap
                .CMap_get_CIDSysInfo()
                .expect("CMap without CIDSystemInfo");
            if csi1.registry != csi2.registry || csi1.ordering != csi2.ordering {
                warn!("CIDSystemInfo mismatched");
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `CMap_set_usecmap`: `ucmap_id` is a cache id; `cmap` must not be
    /// borrowed from the cache (a CMap being parsed is a local, so C's
    /// `cmap == ucmap` check cannot fire).
    pub fn CMap_set_usecmap(&self, cmap: &mut CMap, ucmap_id: i32) -> Result<()> {
        let ucmap = self.CMap_cache_get(ucmap_id)?;
        // Check if ucmap have neccesary information.
        if !self.CMap_is_valid(ucmap)? {
            fatal!("{}: Invalid CMap.", CMAP_DEBUG_STR);
        }
        // CMapName of cmap can be undefined when usecmap is executed in
        // CMap parsing, and CSI too.
        if let Some(name) = &cmap.name {
            if Some(name) == ucmap.name.as_ref() {
                fatal!(
                    "{}: CMap refering itself not allowed: CMap {:?} --> {:?}",
                    CMAP_DEBUG_STR,
                    name,
                    ucmap.name
                );
            }
        }
        if let Some(csi) = &cmap.csi {
            if csi.registry.is_some() && csi.ordering.is_some() {
                let ucsi = ucmap.csi.as_ref().expect("CMap without CIDSystemInfo");
                if csi.registry != ucsi.registry || csi.ordering != ucsi.ordering {
                    fatal!(
                        "{}: CMap {:?} required by {:?} have different CSI.",
                        CMAP_DEBUG_STR,
                        cmap.name,
                        ucmap.name
                    );
                }
            }
        }
        // We must copy codespaceranges.
        for csr in &ucmap.codespace {
            cmap.CMap_add_codespacerange(&csr.code_lo, &csr.code_hi);
        }
        cmap.use_cmap = Some(ucmap_id);
        Ok(())
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
    ) -> Result<()> {
        let mut p = *inpos;
        let mut c: usize = 0;
        let mut count = 0;

        // First handle some special cases:
        if cmap.type_ == CMAP_TYPE_IDENTITY {
            if (*inbytesleft) % 2 != 0 {
                fatal!("{}: Invalid/truncated input string.", CMAP_DEBUG_STR);
            }
            if *outbytesleft < 2 {
                fatal!("{}: Buffer overflow.", CMAP_DEBUG_STR);
            }
            outbuf[*outpos..*outpos + 2].copy_from_slice(&inbuf[*inpos..*inpos + 2]);
            *inpos += 2;
            *outpos += 2;
            *outbytesleft -= 2;
            *inbytesleft -= 2;
            return Ok(());
        }
        let Some(root) = cmap.map_tbl.as_ref() else {
            if let Some(id) = cmap.use_cmap {
                let u = self.CMap_cache_get(id)?;
                self.CMap_decode_char(u, inbuf, inpos, inbytesleft, outbuf, outpos, outbytesleft)?;
            } else {
                // no mapping available in this CMap
                warn!("No mapping available for this character.");
                cmap.handle_undefined(inbuf, inpos, inbytesleft, outbuf, outpos, outbytesleft)?;
            }
            return Ok(());
        };

        let mut t: &Vec<MapDef> = root;
        while count < *inbytesleft {
            c = inbuf[p] as usize;
            p += 1;
            count += 1;
            if LOOKUP_END(t[c].flag) {
                break;
            }
            t = t[c].next.as_ref().unwrap();
        }
        if LOOKUP_CONTINUE(t[c].flag) {
            // need more bytes
            fatal!("{}: Premature end of input string.", CMAP_DEBUG_STR);
        } else if !MAP_DEFINED(t[c].flag) {
            if let Some(id) = cmap.use_cmap {
                let u = self.CMap_cache_get(id)?;
                self.CMap_decode_char(u, inbuf, inpos, inbytesleft, outbuf, outpos, outbytesleft)?;
            } else {
                // no mapping available in this CMap
                warn!("No character mapping available.");
                // We know partial match found up to `count' bytes, but we
                // will not use this information for the sake of simplicity.
                cmap.handle_undefined(inbuf, inpos, inbytesleft, outbuf, outpos, outbytesleft)?;
            }
        } else {
            match MAP_TYPE(t[c].flag) {
                MAP_IS_NOTDEF | MAP_IS_CID | MAP_IS_CODE => {
                    if MAP_TYPE(t[c].flag) == MAP_IS_NOTDEF {
                        warn!("Character mapped to .notdef found.");
                    }
                    let len = t[c].len;
                    if *outbytesleft >= len {
                        let l = len as usize;
                        outbuf[*outpos..*outpos + l].copy_from_slice(&t[c].code[..l]);
                    } else {
                        fatal!("{}: Buffer overflow.", CMAP_DEBUG_STR);
                    }
                    *outpos = (*outpos as isize + len as isize) as usize;
                    *outbytesleft -= len;
                }
                MAP_IS_NAME => {
                    fatal!("{}: CharName mapping not supported.", CMAP_DEBUG_STR);
                }
                _ => {
                    fatal!("{}: Unknown mapping type.", CMAP_DEBUG_STR);
                }
            }
            *inbytesleft -= count;
            *inpos = p;
        }
        Ok(())
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
    ) -> Result<i32> {
        let mut count = 0;
        while *inbytesleft > 0 && *outbytesleft > 0 {
            self.CMap_decode_char(
                cmap,
                inbuf,
                inpos,
                inbytesleft,
                outbuf,
                outpos,
                outbytesleft,
            )?;
            count += 1;
        }
        Ok(count)
    }

    /// `CMap_cache_init`: Identity-H (0) and Identity-V (1).
    pub fn CMap_cache_init(&mut self) -> Result<()> {
        let range_min: [u8; 2] = [0x00, 0x00];
        let range_max: [u8; 2] = [0xff, 0xff];

        if self.cmap.cache.is_some() {
            fatal!("{}: Already initialized.", CMAP_DEBUG_STR);
        }
        let mut cache = Vec::with_capacity(CMAP_CACHE_ALLOC_SIZE as usize);

        // Create Identity mapping
        let mut c0 = CMap::CMap_new();
        c0.CMap_set_name(b"Identity-H");
        c0.CMap_set_type(CMAP_TYPE_IDENTITY);
        c0.CMap_set_wmode(0);
        c0.CMap_set_CIDSysInfo(Some(&crate::cid::CSI_IDENTITY()));
        c0.CMap_add_codespacerange(&range_min, &range_max);
        cache.push(c0);

        let mut c1 = CMap::CMap_new();
        c1.CMap_set_name(b"Identity-V");
        c1.CMap_set_type(CMAP_TYPE_IDENTITY);
        c1.CMap_set_wmode(1);
        c1.CMap_set_CIDSysInfo(Some(&crate::cid::CSI_IDENTITY()));
        c1.CMap_add_codespacerange(&range_min, &range_max);
        cache.push(c1);

        self.cmap.cache = Some(cache);
        Ok(())
    }

    /// `CMap_cache_get`.
    pub fn CMap_cache_get(&self, id: i32) -> Result<&CMap> {
        let Some(cache) = self.cmap.cache.as_ref() else {
            fatal!("{}: CMap cache not initialized.", CMAP_DEBUG_STR);
        };
        if id < 0 || id as usize >= cache.len() {
            fatal!("Invalid CMap ID {}", id);
        }
        Ok(&cache[id as usize])
    }

    /// `CMap_cache_get`, to change a cached CMap (tt_cmap.c adds to one).
    pub fn CMap_cache_get_mut(&mut self, id: i32) -> Result<&mut CMap> {
        let Some(cache) = self.cmap.cache.as_mut() else {
            fatal!("{}: CMap cache not initialized.", CMAP_DEBUG_STR);
        };
        if id < 0 || id as usize >= cache.len() {
            fatal!("Invalid CMap ID {}", id);
        }
        Ok(&mut cache[id as usize])
    }

    /// `CMap_cache_find`: the id of a cached CMap by name, else loads it
    /// (`ResType::Cmap`); -1 if not found. The new id is reserved first
    /// (as C does), the CMap parsed as a local, then stored in its slot.
    pub fn CMap_cache_find(&mut self, cmap_name: &[u8]) -> Result<i32> {
        if self.cmap.cache.is_none() {
            self.CMap_cache_init()?;
        }
        {
            let cache = self.cmap.cache.as_ref().unwrap();
            for (id, cm) in cache.iter().enumerate() {
                // CMapName may be undefined when processing usecmap.
                if let Some(name) = cm.CMap_get_name() {
                    if cmap_name == name {
                        return Ok(id as i32);
                    }
                }
            }
        }

        let Some(mut fp) = self.dpx_open_file(cmap_name, crate::dpxfile::ResType::Cmap)? else {
            return Ok(-1);
        };
        if crate::cmap_read::CMap_parse_check_sig(&mut fp) < 0 {
            return Ok(-1);
        }

        let id = {
            let cache = self.cmap.cache.as_mut().unwrap();
            cache.push(CMap::CMap_new());
            cache.len() - 1
        };
        let mut cmap = CMap::CMap_new();
        if self.CMap_parse(&mut cmap, &mut fp)? < 0 {
            fatal!("{}: Parsing CMap file failed.", CMAP_DEBUG_STR);
        }
        self.cmap.cache.as_mut().unwrap()[id] = cmap;
        Ok(id as i32)
    }

    /// `CMap_cache_close`.
    pub fn CMap_cache_close(&mut self) {
        self.cmap.cache = None;
    }

    /// `CMap_cache_add`: takes ownership; the new id.
    pub fn CMap_cache_add(&mut self, cmap: CMap) -> Result<i32> {
        if !self.CMap_is_valid(&cmap)? {
            fatal!("{}: Invalid CMap.", CMAP_DEBUG_STR);
        }
        let cache = self
            .cmap
            .cache
            .as_mut()
            .expect("CMap cache not initialized");
        for cm in cache.iter() {
            let cmap_name0 = cmap.CMap_get_name();
            let cmap_name1 = cm.CMap_get_name();
            if cmap_name0 == cmap_name1 {
                fatal!("{}: CMap {:?} already defined.", CMAP_DEBUG_STR, cmap_name0);
            }
        }
        cache.push(cmap);
        Ok((cache.len() - 1) as i32)
    }
}
