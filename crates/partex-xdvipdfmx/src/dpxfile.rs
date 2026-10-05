//! dpxfile.c: finding and opening files (placeholder; being written separately).

use crate::prelude::*;

/// dpxfile.h's `DPX_RES_TYPE_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResType {
    Fontmap,
    T1Font,
    TtFont,
    OtFont,
    PkFont,
    DFont,
    Enc,
    Cmap,
    Sfd,
    Agl,
    IccProfile,
    Binary,
    Text,
}

impl Dpx {
    /// `dpx_open_file`.
    pub fn dpx_open_file(&mut self, name: &[u8], ty: ResType) -> Option<MemFile> {
        todo!()
    }
    /// `dpx_find_type1_file`: the path.
    pub fn dpx_find_type1_file(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        todo!()
    }
    /// `dpx_find_truetype_file`: the path.
    pub fn dpx_find_truetype_file(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        todo!()
    }
    /// `dpx_find_opentype_file`: the path.
    pub fn dpx_find_opentype_file(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        todo!()
    }
    /// `dpx_find_dfont_file`: the path.
    pub fn dpx_find_dfont_file(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        todo!()
    }
}
