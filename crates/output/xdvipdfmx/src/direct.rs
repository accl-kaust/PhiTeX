//! A page given as XeTeX's `ship_out` makes it, not as XDV bytes: the
//! engine's walk of a shipped box hands over each DVI operation with its
//! operands ([`Op`]), and the interpreter of dvi.c runs them as
//! `dvi_do_page` runs the opcodes it reads. The registers, the stack,
//! the fonts' widths and every position the device is given are the
//! interpreter's own, so the PDF is the one the page's XDV would give.

use crate::dvi::{FontDef, ScanSpecials};
use crate::pdfcolor::PdfColor;
use crate::pdfdev::PdfCoord;
use crate::prelude::*;
use crate::specials::Again;

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

/// What a page's interpretation takes from the pages before it, beyond
/// the objects ([`Dpx::page_state`]): the values a page written again
/// ([`crate::api::Session::reship`]) is given as the page first found
/// them ([`Dpx::set_page_state`]): the colour stack, the scales and
/// transforms' fixed points, the paper, the background; and what it must
/// find as the last page left it: an annotation or link open across the
/// page break, the `pdfcolorstack`s, the text strings' conversion, the
/// resources and contents added to every page, the extended graphics
/// states open. The page is the page first written when the first part
/// is set as it was before it and comes out as it was after it, and the
/// second is the same before it, after it and after the last page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageState {
    given: Given,
    fixed: Fixed,
}

#[derive(Clone, Debug, PartialEq)]
struct Given {
    colors: (i32, Vec<PdfColor>, Vec<PdfColor>),
    scales: (Vec<PdfCoord>, i32, Vec<u8>),
    coords: (Vec<PdfCoord>, Vec<PdfCoord>),
    paper: [u64; 4],
    landscape: i32,
    background: PdfColor,
}

#[derive(Clone, Debug, PartialEq)]
struct Fixed {
    stacks: Vec<(i32, i32, Vec<Vec<u8>>)>,
    /// The pending `pdf:bann`'s, the open annotation's (its box drawn
    /// to, broken already), the pending `html:` anchor's.
    pending: (bool, Option<(i32, i32)>, i32, bool),
    tounicode: (i32, i32, Vec<Vec<u8>>),
    page: (bool, usize, usize),
    xgstates: usize,
}

impl PageState {
    /// Whether the parts a page must find as the last page left them
    /// are the same.
    #[must_use]
    pub fn same_fixed(&self, o: &PageState) -> bool {
        self.fixed == o.fixed
    }

    /// The parts in which `self` and `o` differ, named.
    #[must_use]
    pub fn differs(&self, o: &PageState) -> String {
        let (a, b) = (&self.given, &o.given);
        let (x, y) = (&self.fixed, &o.fixed);
        let mut out = Vec::new();
        for (d, what) in [
            (a.colors != b.colors, "colour stack"),
            (a.scales != b.scales, "scales"),
            (a.coords != b.coords, "transforms"),
            (a.paper != b.paper || a.landscape != b.landscape, "paper"),
            (a.background != b.background, "background"),
            (x.stacks != y.stacks, "pdfcolorstack"),
            (x.pending != y.pending, "annotation pending"),
            (x.tounicode != y.tounicode, "tounicode"),
            (x.page != y.page, "page resources or bop/eop"),
            (x.xgstates != y.xgstates, "extended graphics states"),
        ] {
            if d {
                out.push(what);
            }
        }
        out.join(", ")
    }
}

impl Dpx {
    /// [`PageState`] now.
    ///
    /// # Errors
    /// None in practice (the objects it reads are strings, names and
    /// streams).
    pub(crate) fn page_state(&self) -> Result<PageState> {
        let cs = &self.color.color_stack;
        let top = usize::try_from(cs.current).unwrap_or(0);
        let upto = |v: &Vec<PdfColor>| v.iter().take(top + 1).cloned().collect::<Vec<_>>();
        let mut stacks = Vec::new();
        for st in &self.misc.stacks {
            let mut items = Vec::new();
            for &o in &st.stack.items {
                items.push(
                    self.o
                        .string_value(o)
                        .map(<[u8]>::to_vec)
                        .unwrap_or_default(),
                );
            }
            stacks.push((st.page, st.direct, items));
        }
        let mut taint = Vec::new();
        if let Some(t) = self.pdfm.cd.taintkeys {
            for x in self.o.array_items(t)?.into_iter().flatten() {
                taint.push(self.o.name_value(x).map(<[u8]>::to_vec).unwrap_or_default());
            }
        }
        let len = |s: Option<Obj>| s.map_or(Ok(0), |s| self.o.stream_length(s));
        let b = &self.doc.breaking_state;
        let ss = &self.session;
        Ok(PageState {
            given: Given {
                colors: (cs.current, upto(&cs.stroke), upto(&cs.fill)),
                scales: (
                    self.xtx.scale_factors.clone(),
                    self.xtx.scale_factor_count,
                    self.xtx.overlay_name.clone(),
                ),
                coords: (
                    self.spc.coords.items.clone(),
                    self.spc.pt_fixee.items.clone(),
                ),
                paper: [
                    ss.paper_width.to_bits(),
                    ss.paper_height.to_bits(),
                    ss.x_offset.to_bits(),
                    ss.y_offset.to_bits(),
                ],
                landscape: ss.landscape_mode,
                background: self.doc.bgcolor.clone(),
            },
            fixed: Fixed {
                stacks,
                pending: (
                    self.pdfm.annot_dict.is_some(),
                    b.annot_dict.map(|_| (b.dirty, b.broken)),
                    self.html.pending_type,
                    self.html.link_dict.is_some(),
                ),
                tounicode: (self.pdfm.cd.cmap_id, self.pdfm.cd.unescape_backslash, taint),
                page: (
                    self.pdfm.pageresources.is_some(),
                    len(self.doc.pdoc.pages.bop)?,
                    len(self.doc.pdoc.pages.eop)?,
                ),
                xgstates: self.draw.xgs_stack.items.len(),
            },
        })
    }

    /// The values of `st` a page is given ([`PageState`]) set.
    pub(crate) fn set_page_state(&mut self, st: &PageState) {
        let g = &st.given;
        let cs = &mut self.color.color_stack;
        cs.current = g.colors.0;
        for (k, c) in g.colors.1.iter().enumerate() {
            if let Some(x) = cs.stroke.get_mut(k) {
                *x = c.clone();
            }
        }
        for (k, c) in g.colors.2.iter().enumerate() {
            if let Some(x) = cs.fill.get_mut(k) {
                *x = c.clone();
            }
        }
        self.xtx.scale_factors.clone_from(&g.scales.0);
        self.xtx.scale_factor_count = g.scales.1;
        self.xtx.overlay_name.clone_from(&g.scales.2);
        self.spc.coords.items.clone_from(&g.coords.0);
        self.spc.pt_fixee.items.clone_from(&g.coords.1);
        let ss = &mut self.session;
        ss.paper_width = f64::from_bits(g.paper[0]);
        ss.paper_height = f64::from_bits(g.paper[1]);
        ss.x_offset = f64::from_bits(g.paper[2]);
        ss.y_offset = f64::from_bits(g.paper[3]);
        ss.landscape_mode = g.landscape;
        self.doc.bgcolor = g.background.clone();
    }

    /// Whether a page of `ops` can be written again ([`Again`]): none of
    /// its specials is one that can't.
    pub(crate) fn can_reship(&mut self, ops: &[Op]) -> Result<bool> {
        for op in ops {
            if let Op::Special(b) = op
                && self.spc_again(b)? == Again::Never
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
