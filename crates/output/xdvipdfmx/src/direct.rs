//! A page given as XeTeX's `ship_out` makes it, not as XDV bytes: the
//! engine's walk of a shipped box hands over each DVI operation with its
//! operands ([`Op`]), and the interpreter of dvi.c runs them as
//! `dvi_do_page` runs the opcodes it reads. The registers, the stack,
//! the fonts' widths and every position the device is given are the
//! interpreter's own, so the PDF is the one the page's XDV would give.

use crate::dvi::{FontDef, ScanSpecials};
use crate::prelude::*;

/// One operation of a page, as the XDV opcode that would carry it.
///
/// A page is `bop` .. `eop` ([`crate::api::Session::page_ops`] does
/// both); the movements are the amounts, whichever of `right`, `w`, `x`
/// (`down`, `y`, `z`) a DVI writer would choose (`w0` moves by `w`, the
/// amount it was set to: the interpreter's position is the same).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// `set_char_c`, `set1`..`set3`: character `c` of the current font,
    /// then right by its width.
    Set(i32),
    /// `set_rule`: a rule, then right by its width.
    SetRule { height: i32, width: i32 },
    /// `put_rule`: a rule, the position unchanged.
    PutRule { height: i32, width: i32 },
    /// `push`.
    Push,
    /// `pop`.
    Pop,
    /// `right`, `w`, `x`.
    Right(i32),
    /// `down`, `y`, `z`.
    Down(i32),
    /// `fnt_def`: TeX's font `id` (the DVI's number) defined, at its
    /// first use in the file; the name as written (XeTeX's cut at a
    /// colon).
    FontDef {
        id: i32,
        checksum: u32,
        size: i32,
        design_size: i32,
        name: Vec<u8>,
    },
    /// XeTeX's `define_native_font`: font `id` and its record after the
    /// number (`make_font_def`: size, flags, the name's length and
    /// bytes, the face index, then colour, extend, slant and embolden as
    /// the flags say).
    NativeFontDef { id: i32, record: Vec<u8> },
    /// `fnt_num`, `fnt1`..: font `id` selected.
    Font(i32),
    /// `xxx`: a `\special`'s bytes.
    Special(Vec<u8>),
    /// XeTeX's `set_glyphs` (`text` none) or `set_text_and_glyphs`: the
    /// run's width, each glyph's offset and id; then right by the width.
    Glyphs {
        text: Option<Vec<u16>>,
        width: i32,
        xy: Vec<(i32, i32)>,
        ids: Vec<u16>,
    },
}

impl Dpx {
    /// `dvi_init` for pages given as [`Op`]s: XeTeX's preamble (id 7,
    /// TeX's units, `dvi_mag`, `comment`) as `get_preamble_dvi_info`
    /// takes it from an XDV piped in, then the scales for the session's
    /// `mag`. The scale (`dvi2pts`).
    pub(crate) fn dvi_init_direct(&mut self, mag: f64, dvi_mag: u32, comment: &[u8]) -> f64 {
        self.dvi.linear = 1;
        self.dvi.pre_id_byte = i32::from(crate::dvi::XDV_ID);
        self.conf.compat_mode = crate::ctx::CompatMode::Xdv;
        self.dvi.is_ptex = 0;
        self.dvi.dvi_info.unit_num = 25_400_000;
        self.dvi.dvi_info.unit_den = 473_628_672;
        self.dvi.dvi_info.mag = dvi_mag;
        self.dvi.dvi_info.comment = comment[..comment.len().min(255)].to_vec();
        self.dvi.num_pages = 0x7FF_FFFF;
        self.dvi.total_mag = f64::from(self.dvi.dvi_info.mag) / 1000.0 * mag;
        self.dvi.dvi2pts = f64::from(self.dvi.dvi_info.unit_num)
            / f64::from(self.dvi.dvi_info.unit_den)
            * (72.0 / 254000.0);
        self.clear_state();
        self.dvi.dvi_page_buffer = vec![0; crate::dvi::DVI_PAGE_BUF_CHUNK as usize];
        self.dvi.dvi_page_buf_size = crate::dvi::DVI_PAGE_BUF_CHUNK;
        self.dvi.dvi2pts
    }

    /// `dvi_scan_specials` over a page's operations: its specials scanned
    /// in order and its fonts defined, as the XDV's page is buffered.
    pub(crate) fn dvi_scan_ops(
        &mut self,
        page_no: i32,
        sp: &mut ScanSpecials,
        ops: &[Op],
    ) -> Result<()> {
        if page_no == self.dvi.buffered_page || self.dvi.num_pages == 0 {
            return Ok(());
        }
        self.dvi.buffered_page = page_no;
        for op in ops {
            match op {
                Op::Special(s) => {
                    self.scan_special(sp, s)?;
                }
                Op::FontDef {
                    id,
                    checksum,
                    size,
                    design_size,
                    name,
                } => {
                    if *size < 0 {
                        crate::fatal!("Bad DVI: negative point_size: {}", size);
                    }
                    if *design_size < 0 {
                        crate::fatal!("Bad DVI: negative design_size: {}", design_size);
                    }
                    self.define_font(
                        *id,
                        *checksum,
                        *size as u32,
                        *design_size as u32,
                        name.clone(),
                    )?;
                }
                Op::NativeFontDef { id, record } => {
                    let mut f = MemFile::new(Arc::from(record.as_slice()), b"native font");
                    let d: FontDef = Self::native_font_record(*id, &mut f)?;
                    self.dvi.def_fonts.push(d);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `dvi_do_page` over a page's operations.
    pub(crate) fn dvi_do_ops(
        &mut self,
        ops: &[Op],
        page_paper_height: f64,
        hmargin: f64,
        vmargin: f64,
    ) -> Result<()> {
        self.dvi.dvi_page_buf_index = 0;
        self.dvi.dev_origin_x = hmargin;
        self.dvi.dev_origin_y = page_paper_height - vmargin;
        self.dvi.dvi_stack_depth = 0;
        self.do_bop()?;
        for op in ops {
            match op {
                Op::Set(c) => self.dvi_set(*c)?,
                Op::SetRule { height, width } => self.setrule(*height, *width)?,
                Op::PutRule { height, width } => self.putrule(*height, *width)?,
                Op::Push => {
                    self.dvi_push()?;
                    self.dvi_mark_depth()?;
                }
                Op::Pop => {
                    self.dvi_pop()?;
                    self.dvi_mark_depth()?;
                }
                Op::Right(x) => self.dvi_right(*x),
                Op::Down(y) => self.dvi_down(*y),
                Op::FontDef { .. } | Op::NativeFontDef { .. } => {}
                Op::Font(id) => self.do_fnt(*id)?,
                Op::Special(s) => self.dvi_do_special(s)?,
                Op::Glyphs {
                    text,
                    width,
                    xy,
                    ids,
                } => {
                    if self.dvi.current_font < 0 {
                        crate::fatal!("No font selected!");
                    }
                    if let Some(u) = text {
                        self.pdf_dev_begin_actualtext(u)?;
                        self.glyph_runs_span(u);
                    }
                    self.glyphs(text.is_some(), *width, xy, ids)?;
                }
            }
        }
        self.do_eop()
    }
}
