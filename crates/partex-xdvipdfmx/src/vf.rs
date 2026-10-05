//! vf.c, vf.h: virtual fonts.
//!
//! The loaded VFs are `self.vf.vf_fonts` (count = `len()`), indexed by the
//! id `vf_locate_font` returns. Character packets are `Rc<[u8]>` so that
//! `vf_set_char` can hold one while it runs the DVI interpreter (which may
//! load more VFs: C's "this code needs to be able to recurse").
//! Packet readers take the packet slice (ending at C's `end`) and a
//! position (C's `*start`).

use crate::pdfdev::Spt;
use crate::prelude::*;

/// `VF_ALLOC_SIZE`.
pub const VF_ALLOC_SIZE: u32 = 16;
/// `VF_ID`.
pub const VF_ID: u8 = 202;
/// `FIX_WORD_BASE`.
pub const FIX_WORD_BASE: f64 = 1048576.0;
/// `TEXPT2PT`.
pub const TEXPT2PT: f64 = 72.0 / 72.27;
/// `FW2PT`.
pub const FW2PT: f64 = TEXPT2PT / FIX_WORD_BASE;
/// `CHAR_INDEX_MIN`: characters from here are looked up via `idx_to_char`.
pub const CHAR_INDEX_MIN: i32 = 0x40000;

/// `struct font_def`: a font the VF uses.
#[derive(Clone, Debug, Default)]
pub struct FontDef {
    /// The id used inside the VF file.
    pub font_id: i32,
    pub checksum: u32,
    pub size: u32,
    pub design_size: u32,
    pub directory: Option<Vec<u8>>,
    pub name: Option<Vec<u8>>,
    /// Returned by the TFM module.
    pub tfm_id: i32,
    /// Returned by the DEV module (`dvi_locate_font`).
    pub dev_id: i32,
}

/// `struct vf`.
#[derive(Clone, Debug, Default)]
pub struct Vf {
    pub tex_name: Vec<u8>,
    pub ptsize: Spt,
    /// A fixword-pts quantity.
    pub design_size: u32,
    /// `dev_fonts` (`num_dev_fonts` = `len()`).
    pub dev_fonts: Vec<FontDef>,
    /// `max_dev_fonts`.
    pub max_dev_fonts: i32,
    /// `ch_pkt` (`num_chars` = `len()`).
    pub ch_pkt: Vec<Option<Rc<[u8]>>>,
    pub message_flag: u8,
    pub pkt_len: Vec<u32>,
    pub idx_to_char: Vec<u32>,
    pub max_idx: u32,
}

/// vf.c's globals.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `vf_fonts` (`num_vf_fonts` = `len()`).
    pub vf_fonts: Vec<Vf>,
    /// `max_vf_fonts`.
    pub max_vf_fonts: i32,
}

/// `resize_one_vf_font` (static).
fn resize_one_vf_font(a_vf: &mut Vf, size: u32) {
    todo!()
}

/// `unsigned_byte` (static).
fn unsigned_byte(s: &[u8], start: &mut usize) -> i32 {
    todo!()
}

/// `get_pkt_signed_num` (static): `num + 1` bytes.
fn get_pkt_signed_num(s: &[u8], start: &mut usize, num: u8) -> i32 {
    todo!()
}

/// `get_pkt_unsigned_num` (static).
fn get_pkt_unsigned_num(s: &[u8], start: &mut usize, num: u8) -> i32 {
    todo!()
}

impl Dpx {
    /// `read_header` (static).
    fn read_header(&mut self, vf_file: &mut MemFile, thisfont: i32) {
        todo!()
    }

    /// `resize_vf_fonts` (static).
    fn resize_vf_fonts(&mut self, size: i32) {
        todo!()
    }

    /// `read_a_char_def` (static).
    fn read_a_char_def(&mut self, vf_file: &mut MemFile, thisfont: i32, pkt_len: u32, ch: u32) {
        todo!()
    }

    /// `read_a_font_def` (static).
    fn read_a_font_def(&mut self, vf_file: &mut MemFile, font_id: i32, thisfont: i32) {
        todo!()
    }

    /// `process_vf_file` (static).
    fn process_vf_file(&mut self, vf_file: &mut MemFile, thisfont: i32) {
        todo!()
    }

    /// `vf_locate_font`: the vf id, or -1.
    pub fn vf_locate_font(&mut self, tex_name: &[u8], ptsize: Spt) -> i32 {
        todo!()
    }

    /// `vf_putrule` (static).
    fn vf_putrule(&mut self, s: &[u8], start: &mut usize, ptsize: Spt) {
        todo!()
    }

    /// `vf_setrule` (static).
    fn vf_setrule(&mut self, s: &[u8], start: &mut usize, ptsize: Spt) {
        todo!()
    }

    /// `vf_fnt` (static).
    fn vf_fnt(&mut self, font_id: i32, vf_font: i32) {
        todo!()
    }

    /// `vf_xxx` (static): a special (or a `Warning:`) in a packet.
    fn vf_xxx(&mut self, len: i32, s: &[u8], start: &mut usize) {
        todo!()
    }

    /// `vf_set_char`.
    pub fn vf_set_char(&mut self, ch: i32, vf_font: i32) {
        todo!()
    }

    /// `vf_close_all_fonts`.
    pub fn vf_close_all_fonts(&mut self) {
        todo!()
    }
}
