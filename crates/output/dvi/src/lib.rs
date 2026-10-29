//! DVI output (tex.web §583–§642) and XeTeX's XDV variant (native glyph
//! runs, pictures), from shipped boxes to bytes.
//!
//! [`DviWriter`] is `ship_out` (§638–§640) with `hlist_out` and
//! `vlist_out` (§619–§637) over [`phitex_typeset`] nodes, writing the
//! bytes web2c's TeX writes. TeX's output buffer is not needed, but its
//! bookkeeping is: `movement` may only rewrite bytes still in the buffer
//! (§611) and `dvi_pop` only cancels a `push` in it (§601), so the
//! writer keeps `dvi_ptr`, `dvi_limit`, `dvi_offset` and `dvi_gone` as
//! counters over one growing vector, with web2c's `dvi_buf_size`.

// tex.web's octal constants are kept so that the code reads against the
// cited sections.
#![allow(clippy::unreadable_literal, clippy::many_single_char_names)]

use std::collections::BTreeMap;

use phitex_pdf::GlyphTap;
pub use phitex_xdvipdfmx::api::Op;

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;
use phitex_typeset::glue::{GlueSign, Order};
use phitex_typeset::node::{
    BoxKind, BoxNode, GlueKind, GlueNode, KernKind, MathKind, Native, NativeKind, Node, RUNNING,
};

/// DVI opcodes (§586).
pub mod op {
    /// `set_char_0`.
    pub const SET_CHAR_0: u8 = 0;
    /// `set1`.
    pub const SET1: u8 = 128;
    /// `set_rule`.
    pub const SET_RULE: u8 = 132;
    /// `put_rule`.
    pub const PUT_RULE: u8 = 137;
    /// `bop`.
    pub const BOP: u8 = 139;
    /// `eop`.
    pub const EOP: u8 = 140;
    /// `push`.
    pub const PUSH: u8 = 141;
    /// `pop`.
    pub const POP: u8 = 142;
    /// `right1`.
    pub const RIGHT1: u8 = 143;
    /// `w0`.
    pub const W0: u8 = 147;
    /// `w1`.
    pub const W1: u8 = 148;
    /// `x0`.
    pub const X0: u8 = 152;
    /// `x1`.
    pub const X1: u8 = 153;
    /// `down1`.
    pub const DOWN1: u8 = 157;
    /// `y0`.
    pub const Y0: u8 = 161;
    /// `y1`.
    pub const Y1: u8 = 162;
    /// `z0`.
    pub const Z0: u8 = 166;
    /// `z1`.
    pub const Z1: u8 = 167;
    /// `fnt_num_0`.
    pub const FNT_NUM_0: u8 = 171;
    /// `fnt1`.
    pub const FNT1: u8 = 235;
    /// `xxx1`.
    pub const XXX1: u8 = 239;
    /// `xxx4`.
    pub const XXX4: u8 = 242;
    /// `fnt_def1`.
    pub const FNT_DEF1: u8 = 243;
    /// `pre`.
    pub const PRE: u8 = 247;
    /// `post`.
    pub const POST: u8 = 248;
    /// `post_post`.
    pub const POST_POST: u8 = 249;
    /// `id_byte` (§587).
    pub const ID_BYTE: u8 = 2;
    /// XeTeX's `id_byte`: an XDV file.
    pub const XDV_ID_BYTE: u8 = 7;
    /// XeTeX's `define_native_font`.
    pub const DEFINE_NATIVE_FONT: u8 = 252;
    /// XeTeX's `set_glyphs`.
    pub const SET_GLYPHS: u8 = 253;
    /// XeTeX's `set_text_and_glyphs`.
    pub const SET_TEXT_AND_GLYPHS: u8 = 254;
}

/// web2c's default `dvi_buf_size` (texmf.cnf), a multiple of 8.
pub const DVI_BUF_SIZE: usize = 16384;

/// What the DVI file says about a font (§602), and its widths for
/// `hlist_out` (§620).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontDef {
    /// `font_check`.
    pub checksum: [u8; 4],
    /// `font_size`.
    pub size: Scaled,
    /// `font_dsize`.
    pub design_size: Scaled,
    /// `font_area`.
    pub area: Vec<u8>,
    /// `font_name`.
    pub name: Vec<u8>,
    /// `char_width` by character code; `None` (or past the end) where
    /// the character doesn't exist, which web2c's TeX doesn't output
    /// (`tex.ch`, `MLTeX` disabled).
    pub widths: Vec<Option<Scaled>>,
    /// A XeTeX native font's definition after its number
    /// (`make_font_def`: size, flags, path, face index, ...): written as
    /// `define_native_font` instead of `fnt_def`.
    pub native: Option<Vec<u8>>,
}

/// The direction of a [`DviWriter::movement`] (§607): `o` is `down1` or
/// `right1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// `down1`, `y`, `z`.
    Down,
    /// `right1`, `w`, `x`.
    Right,
}

impl Dir {
    fn base(self) -> u8 {
        match self {
            Dir::Down => op::DOWN1,
            Dir::Right => op::RIGHT1,
        }
    }
}

/// Why a page was not shipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShipError {
    /// "Huge page cannot be shipped out" (§641).
    HugePage,
}

// §608: the `info` settings of a movement entry
const Y_HERE: u8 = 1;
const Z_HERE: u8 = 2;
const YZ_OK: u8 = 3;
const Y_OK: u8 = 4;
const Z_OK: u8 = 5;
const D_FIXED: u8 = 6;
// §611: the search states
const NONE_SEEN: u8 = 0;
const Y_SEEN: u8 = 6;
const Z_SEEN: u8 = 12;

/// An entry of the down or right stack (§605).
#[derive(Clone, Copy, Debug)]
struct Movement {
    width: Scaled,
    location: usize,
    info: u8,
}

/// `max_dimen` (§421).
const MAX_DIMEN: Scaled = 0o7777777777;
/// `billion` (§625).
const BILLION: f64 = 1_000_000_000.0;

/// A box being output: what `hlist_out` and `vlist_out` keep of
/// `this_box` and their glue locals (§619, §629).
struct Frame {
    width: Scaled,
    height: Scaled,
    depth: Scaled,
    g_sign: GlueSign,
    g_order: Order,
    glue_set: f64,
    cur_g: Scaled,
    cur_glue: f64,
    /// `base_line` in an hlist, `top_edge` in a vlist.
    base: Scaled,
    left_edge: Scaled,
}

impl Frame {
    fn new(b: &BoxNode, base: Scaled, left_edge: Scaled) -> Frame {
        Frame {
            width: b.width,
            height: b.height,
            depth: b.depth,
            g_sign: b.glue.sign,
            g_order: b.glue.order,
            glue_set: b.glue.ratio,
            cur_g: 0,
            cur_glue: 0.0,
            base,
            left_edge,
        }
    }

    /// The glue's size as set, `width(g)-cur_g` advanced to the new
    /// `cur_g` (§625, §634).
    fn glue(&mut self, g: &GlueNode) -> Scaled {
        let spec = g.spec;
        let mut size = spec.width - self.cur_g;
        match self.g_sign {
            GlueSign::Stretching if spec.stretch_order == self.g_order => {
                self.cur_glue += f64::from(spec.stretch);
                self.cur_g = vet_round(self.glue_set * self.cur_glue);
            }
            GlueSign::Shrinking if spec.shrink_order == self.g_order => {
                self.cur_glue -= f64::from(spec.shrink);
                self.cur_g = vet_round(self.glue_set * self.cur_glue);
            }
            _ => {}
        }
        size += self.cur_g;
        size
    }
}

/// An item of an hlist being output with TeX--XeT: a node, e-TeX's
/// edge node (a change of direction: `cur_h` moves by `width`, the new
/// left edge is `dist` beyond), or leaders glue whose width `reverse`
/// has fixed.
enum Lr {
    N(Node),
    Edge { dir: u8, width: Scaled, dist: Scaled },
    Fixed(GlueNode),
}

/// `list`'s nodes, a discretionary's replacement text in its place (the
/// nodes TeX's list has after the discretionary node).
fn flatten_into(list: &[Node], q: &mut std::collections::VecDeque<Lr>) {
    for n in list {
        match n {
            Node::Disc(d) => flatten_into(&d.replace, q),
            n => q.push_back(Lr::N(n.clone())),
        }
    }
}

/// `vet_glue(float(glue_set(this_box))*cur_glue)` then
/// `cur_g:=round(glue_temp)` (§625), in doubles as web2c computes it
/// (`glueratio` and `real` are C `double`; `round` is `zround`).
#[allow(clippy::cast_possible_truncation)]
fn vet_round(glue_temp: f64) -> Scaled {
    let t = glue_temp.clamp(-BILLION, BILLION);
    // web2c's zround: (integer)(r+0.5) for r>=0, (integer)(r-0.5) else
    if t >= 0.0 {
        (t + 0.5) as Scaled
    } else {
        (t - 0.5) as Scaled
    }
}

/// The DVI writer: the state `ship_out` keeps across pages (§595, §603,
/// §605, §616, §646). A clone goes on from the same state: what an
/// edit's file keeps of the pages before the first that changed.
#[derive(Clone)]
pub struct DviWriter {
    out: Vec<u8>,
    /// `dvi_offset+dvi_ptr`: the bytes written, kept or not.
    loc: usize,
    /// Whether the bytes are kept (none for a writer that only says its
    /// operations, [`DviWriter::new_xdv_ops`]).
    keep: bool,
    /// The operations written since the last [`DviWriter::take_ops`], as
    /// the XDV's opcodes would carry them (none: not said).
    ops: Option<Vec<Op>>,
    buf_size: usize,
    dvi_ptr: usize,
    dvi_limit: usize,
    dvi_offset: usize,
    dvi_gone: usize,
    mag: i32,
    /// `h_offset` (§236), the left margin of every page.
    pub h_offset: Scaled,
    /// `v_offset` (§236), the top margin of every page.
    pub v_offset: Scaled,
    total_pages: i32,
    max_v: Scaled,
    max_h: Scaled,
    max_push: i32,
    last_bop: i32,
    fonts: BTreeMap<FontId, FontDef>,
    /// The fonts' DVI numbers plus one by id, where the ids are not the
    /// numbers ([`DviWriter::set_font_numbers`]); none: an id is its
    /// number.
    nums: Option<std::sync::Arc<BTreeMap<FontId, u32>>>,
    down: Vec<Movement>,
    right: Vec<Movement>,
    dvi_h: Scaled,
    dvi_v: Scaled,
    cur_h: Scaled,
    cur_v: Scaled,
    dvi_f: Option<FontId>,
    cur_s: i32,
    doing_leaders: bool,
    /// e-TeX's `cur_dir`: 1 inside right-to-left text (TeX--XeT).
    cur_dir: u8,
    /// While [`DviWriter::ship_out_kept`] ships a page: its first byte,
    /// the buffer-dependent decisions so far and the fonts it uses.
    rec: Option<Recording>,
    /// XeTeX's XDV file ([`DviWriter::new_xdv`]).
    xdv: bool,
    /// The next page's `\pdfpagewidth` and `\pdfpageheight` (XDV: the
    /// `pdf:pagesize` special each page begins with).
    pub page_size: (Scaled, Scaled),
    /// The tap told the glyphs written ([`DviWriter::set_tap`]).
    tap: Tap,
}

/// The writer's tap (`phitex_pdf::GlyphTap`): outside the bytes, and
/// not cloned (a clone tells none).
#[derive(Default)]
struct Tap(Option<Box<dyn GlyphTap>>);

impl Clone for Tap {
    fn clone(&self) -> Tap {
        Tap(None)
    }
}

/// dvipdfmx's origin, one inch from the page's top and left edges
/// (TeX's `one_true_inch`): the tap's positions are from the corner.
const ONE_TRUE_INCH: Scaled = 4736287;

/// The output buffer's state at a byte (§595–§598): `dvi_ptr`,
/// `dvi_limit` and how many bytes back `dvi_gone` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Phase {
    ptr: usize,
    limit: usize,
    back: usize,
}

/// A decision of the writer that reads the buffer's state, at a byte
/// counted from the page's first: a cancelled `push` (§601, `dvi_ptr>0`),
/// a `pop` that could not cancel its `push` because the buffer had just
/// been emptied (`dvi_ptr=0`), and whether a movement's entry was gone
/// (§611).
#[derive(Clone, Copy, Debug)]
enum Ev {
    Cancel(usize),
    Zero(usize),
    Gone { at: usize, e: usize, gone: bool },
}

#[derive(Clone)]
struct Recording {
    start: usize,
    tape: Vec<Ev>,
    used: Vec<FontId>,
    new: Vec<FontId>,
}

/// A font a kept page uses: its number, `font_check`, `font_size`,
/// `font_dsize`, area and name.
type UsedFont = (FontId, [u8; 4], Scaled, Scaled, Vec<u8>, Vec<u8>);

/// A page's bytes as [`DviWriter::ship_out_kept`] wrote them, with what
/// they depend on outside the page: the buffer's state at its first byte
/// (and the decisions that read it), the fonts it used and which of them
/// it defined, the margins; and what the postamble needs of it.
#[derive(Debug)]
pub struct PageBytes {
    bytes: Vec<u8>,
    start: Phase,
    end: Phase,
    tape: Vec<Ev>,
    used: Vec<UsedFont>,
    new: Vec<(FontId, FontDef)>,
    offsets: (Scaled, Scaled),
    max_v: Scaled,
    max_h: Scaled,
    max_push: i32,
}

impl PageBytes {
    /// The page's bytes, `bop` to `eop`.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Whether the entry at `e` is gone at `location` (§611), noted for the
/// page being recorded.
fn is_gone(rec: &mut Option<Recording>, location: usize, e: usize, gone: usize) -> bool {
    if let Some(r) = rec {
        r.tape.push(Ev::Gone {
            at: location - r.start,
            e: e - r.start,
            gone: e < gone,
        });
    }
    e < gone
}

/// The buffer's state after `len` bytes from `p` with the page's
/// decisions `tape`, when every decision comes out as it did; `None`
/// when one does not (§598, §601, §611).
fn replay(size: usize, p: Phase, tape: &[Ev], len: usize) -> Option<Phase> {
    let half = size / 2;
    let (mut ptr, mut limit, mut loc) = (p.ptr, p.limit, 0usize);
    let mut gone = -isize::try_from(p.back).ok()?;
    let to = |at: usize, ptr: &mut usize, limit: &mut usize, loc: &mut usize, gone: &mut isize| {
        let mut n = at - *loc;
        while n > 0 {
            let step = n.min(*limit - *ptr);
            *ptr += step;
            n -= step;
            if *ptr == *limit {
                if *limit == size {
                    *limit = half;
                    *ptr = 0;
                } else {
                    *limit = size;
                }
                *gone += isize::try_from(half).unwrap_or(isize::MAX);
            }
        }
        *loc = at;
    };
    for &ev in tape {
        match ev {
            Ev::Cancel(at) => {
                to(at, &mut ptr, &mut limit, &mut loc, &mut gone);
                if ptr == 0 {
                    return None;
                }
                ptr -= 1;
                loc -= 1;
            }
            Ev::Zero(at) => {
                to(at, &mut ptr, &mut limit, &mut loc, &mut gone);
                if ptr != 0 {
                    return None;
                }
            }
            Ev::Gone { at, e, gone: g } => {
                to(at, &mut ptr, &mut limit, &mut loc, &mut gone);
                if (isize::try_from(e).ok()? < gone) != g {
                    return None;
                }
            }
        }
    }
    to(len, &mut ptr, &mut limit, &mut loc, &mut gone);
    Some(Phase {
        ptr,
        limit,
        back: usize::try_from(isize::try_from(len).ok()? - gone).ok()?,
    })
}

/// A whatsit's handle and whether leaders are being output; the answer
/// is a `\special`'s bytes, or `None` (§1366–§1368). `\write`,
/// `\openout` and `\closeout` are the caller's work at this moment,
/// skipped inside leaders (§1374).
pub type Whatsits<'a> = dyn FnMut(u32, bool) -> Option<Vec<u8>> + 'a;

impl DviWriter {
    /// A writer whose preamble (§617) is `pre`, `id_byte`, the ratio
    /// 25400000/473628672, `mag` and `comment` (TeX's is
    /// `" TeX output YYYY.MM.DD:HHMM"`, at most 255 bytes kept).
    #[must_use]
    pub fn new(mag: i32, comment: &str) -> DviWriter {
        DviWriter::with_buf_size(mag, comment, DVI_BUF_SIZE)
    }

    /// [`DviWriter::new`] with another `dvi_buf_size` (a multiple of 8).
    #[must_use]
    pub fn with_buf_size(mag: i32, comment: &str, buf_size: usize) -> DviWriter {
        Self::make(mag, comment, buf_size, false)
    }

    /// XeTeX's writer (xetex.web §583 ff.): an XDV file, `id_byte` 7,
    /// each page beginning with its `pdf:pagesize` special, native fonts
    /// and glyph runs.
    #[must_use]
    pub fn new_xdv(mag: i32, comment: &str) -> DviWriter {
        Self::make(mag, comment, DVI_BUF_SIZE, true)
    }

    fn id_byte(&self) -> u8 {
        if self.xdv {
            op::XDV_ID_BYTE
        } else {
            op::ID_BYTE
        }
    }

    /// XeTeX's writer that keeps no bytes and says the operations it
    /// writes instead ([`DviWriter::take_ops`]): what xdvipdfmx's
    /// interpreter runs for a page when the engine writes the PDF
    /// itself. Every decision the writer makes is as [`DviWriter::new_xdv`]
    /// makes it (its buffer's state is counted, not kept), so the
    /// operations are the opcodes of the XDV it would write.
    #[must_use]
    pub fn new_xdv_ops(mag: i32, comment: &str) -> DviWriter {
        let mut w = Self::make_with(mag, comment, DVI_BUF_SIZE, true, false);
        w.ops = Some(Vec::new());
        w
    }

    /// The operations written since the last call (a page's, after
    /// [`DviWriter::ship_out`]).
    pub fn take_ops(&mut self) -> Vec<Op> {
        self.ops.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Operation `o` said.
    fn op(&mut self, o: Op) {
        if let Some(ops) = &mut self.ops {
            ops.push(o);
        }
    }

    fn make(mag: i32, comment: &str, buf_size: usize, xdv: bool) -> DviWriter {
        Self::make_with(mag, comment, buf_size, xdv, true)
    }

    fn make_with(mag: i32, comment: &str, buf_size: usize, xdv: bool, keep: bool) -> DviWriter {
        let mut w = DviWriter {
            loc: 0,
            keep,
            ops: None,
            xdv,
            page_size: (0, 0),
            out: Vec::new(),
            buf_size,
            dvi_ptr: 0,
            dvi_limit: buf_size,
            dvi_offset: 0,
            dvi_gone: 0,
            mag,
            h_offset: 0,
            v_offset: 0,
            total_pages: 0,
            max_v: 0,
            max_h: 0,
            max_push: 0,
            last_bop: -1,
            fonts: BTreeMap::new(),
            nums: None,
            down: Vec::new(),
            right: Vec::new(),
            dvi_h: 0,
            dvi_v: 0,
            cur_h: 0,
            cur_v: 0,
            dvi_f: None,
            cur_s: -1,
            doing_leaders: false,
            cur_dir: 0,
            rec: None,
            tap: Tap(None),
        };
        // §617
        w.dvi_out(op::PRE);
        w.dvi_out(w.id_byte());
        w.dvi_four(25400000);
        w.dvi_four(473628672);
        w.dvi_four(mag);
        let c = &comment.as_bytes()[..comment.len().min(255)];
        w.dvi_out(u8::try_from(c.len()).unwrap_or(255));
        for &b in c {
            w.dvi_out(b);
        }
        w
    }

    /// Sets the tap the writer tells the glyphs it writes (`None`:
    /// none), as dvipdfmx's PDF shows them, and gives back the one set
    /// before. The bytes are the same either way.
    pub fn set_tap(&mut self, t: Option<Box<dyn GlyphTap>>) -> Option<Box<dyn GlyphTap>> {
        std::mem::replace(&mut self.tap.0, t)
    }

    /// The bytes so far.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.out
    }

    /// `dvi_offset+dvi_ptr`: the location of the next byte (§601).
    #[must_use]
    pub fn location(&self) -> usize {
        self.loc
    }

    /// `dvi_out` (§598) with `dvi_swap`'s bookkeeping (§598): a full half
    /// of the buffer is gone and can no longer change.
    pub fn dvi_out(&mut self, b: u8) {
        if self.keep {
            self.out.push(b);
        }
        self.loc += 1;
        self.dvi_ptr += 1;
        if self.dvi_ptr == self.dvi_limit {
            let half = self.buf_size / 2;
            if self.dvi_limit == self.buf_size {
                self.dvi_limit = half;
                self.dvi_offset += self.buf_size;
                self.dvi_ptr = 0;
            } else {
                self.dvi_limit = self.buf_size;
            }
            self.dvi_gone += half;
        }
    }

    /// `dvi_four` (§600): four bytes, two's complement.
    pub fn dvi_four(&mut self, x: i32) {
        for b in x.to_be_bytes() {
            self.dvi_out(b);
        }
    }

    /// `dvi_pop(l)` (§601): a `pop`, or cancel the `push` at `l-1` when
    /// nothing followed it and it is still in the buffer.
    pub fn dvi_pop(&mut self, l: usize) {
        if l == self.dvi_offset + self.dvi_ptr
            && let Some(r) = &mut self.rec
        {
            let at = l - r.start;
            r.tape.push(if self.dvi_ptr > 0 {
                Ev::Cancel(at)
            } else {
                Ev::Zero(at)
            });
        }
        if l == self.dvi_offset + self.dvi_ptr && self.dvi_ptr > 0 {
            self.dvi_ptr -= 1;
            if self.keep {
                self.out.pop();
            }
            self.loc -= 1;
            // (nothing was written after the `push`: it was the last
            // operation)
            if let Some(ops) = &mut self.ops {
                let push = ops.pop();
                debug_assert_eq!(push, Some(Op::Push));
            }
        } else {
            self.dvi_out(op::POP);
            self.op(Op::Pop);
        }
    }

    /// The fonts' numbers (TeX's `f`, the DVI's `f - 1`) by the ids the
    /// boxes give them, where they differ: a client whose fonts are known
    /// by keys numbers them in the order TeX loaded them.
    pub fn set_font_numbers(&mut self, nums: std::sync::Arc<BTreeMap<FontId, u32>>) {
        self.nums = Some(nums);
    }

    /// Font `f`'s number (TeX's `f`).
    fn num(&self, f: FontId) -> FontId {
        self.nums
            .as_ref()
            .and_then(|m| m.get(&f).copied())
            .unwrap_or(f)
    }

    /// `dvi_font_def(f)` (§602; web2c's two-byte form above 256 fonts).
    fn dvi_font_def(&mut self, id: FontId) {
        let f = self.num(id);
        let n = f - 1;
        if let Some(def) = self.fonts[&id].native.clone() {
            // XeTeX's `dvi_native_font_def`
            if self.ops.is_some() {
                self.op(Op::NativeFontDef {
                    id: n.cast_signed(),
                    record: def.clone(),
                });
            }
            self.dvi_out(op::DEFINE_NATIVE_FONT);
            self.dvi_four(n.cast_signed());
            for b in def {
                self.dvi_out(b);
            }
            return;
        }
        if f <= 256 {
            self.dvi_out(op::FNT_DEF1);
            self.dvi_out(low(n));
        } else {
            self.dvi_out(op::FNT_DEF1 + 1);
            self.dvi_out(low(n / 0o400));
            self.dvi_out(low(n));
        }
        let d = self.fonts[&id].clone();
        for b in d.checksum {
            self.dvi_out(b);
        }
        self.dvi_four(d.size);
        self.dvi_four(d.design_size);
        // (XeTeX: the name to its first colon, a mapping option cut)
        let name = if self.xdv {
            d.name
                .iter()
                .position(|&b| b == b':')
                .filter(|&k| k > 0)
                .map_or(&d.name[..], |k| &d.name[..k])
        } else {
            &d.name[..]
        };
        if self.ops.is_some() {
            let name = name.to_vec();
            self.op(Op::FontDef {
                id: n.cast_signed(),
                checksum: u32::from_be_bytes(d.checksum),
                size: d.size,
                design_size: d.design_size,
                name,
            });
        }
        self.dvi_out(low(u32::try_from(d.area.len()).unwrap_or(0)));
        self.dvi_out(low(u32::try_from(name.len()).unwrap_or(0)));
        // §603
        for &b in d.area.iter().chain(name) {
            self.dvi_out(b);
        }
    }

    /// `movement(w, o)` (§607): a `down`/`right` command for `w`, or a
    /// `y0`/`z0` (`w0`/`x0`) reusing an earlier one (§608–§615).
    #[allow(clippy::too_many_lines)]
    pub fn movement(&mut self, w: Scaled, dir: Dir) {
        self.op(match dir {
            Dir::Down => Op::Down(w),
            Dir::Right => Op::Right(w),
        });
        let o = dir.base();
        let location = self.location();
        let gone = self.dvi_gone;
        let stack = match dir {
            Dir::Down => &mut self.down,
            Dir::Right => &mut self.right,
        };
        stack.push(Movement {
            width: w,
            location,
            info: 0,
        });
        let q = stack.len() - 1;
        // §611: look at the other entries, top down
        let mut mstate = NONE_SEEN;
        let mut found: Option<usize> = None;
        for p in (0..q).rev() {
            let e = stack[p];
            if e.width == w {
                // §612
                match mstate + e.info {
                    s if s == NONE_SEEN + YZ_OK
                        || s == NONE_SEEN + Y_OK
                        || s == Z_SEEN + YZ_OK
                        || s == Z_SEEN + Y_OK =>
                    {
                        if is_gone(&mut self.rec, location, e.location, gone) {
                            break;
                        }
                        // §613
                        if self.keep {
                            self.out[e.location] += op::Y1 - op::DOWN1;
                        }
                        stack[p].info = Y_HERE;
                        found = Some(p);
                        break;
                    }
                    s if s == NONE_SEEN + Z_OK || s == Y_SEEN + YZ_OK || s == Y_SEEN + Z_OK => {
                        if is_gone(&mut self.rec, location, e.location, gone) {
                            break;
                        }
                        // §614
                        if self.keep {
                            self.out[e.location] += op::Z1 - op::DOWN1;
                        }
                        stack[p].info = Z_HERE;
                        found = Some(p);
                        break;
                    }
                    s if s == NONE_SEEN + Y_HERE
                        || s == NONE_SEEN + Z_HERE
                        || s == Y_SEEN + Z_HERE
                        || s == Z_SEEN + Y_HERE =>
                    {
                        found = Some(p);
                        break;
                    }
                    _ => {}
                }
            } else {
                match mstate + e.info {
                    s if s == NONE_SEEN + Y_HERE => mstate = Y_SEEN,
                    s if s == NONE_SEEN + Z_HERE => mstate = Z_SEEN,
                    s if s == Y_SEEN + Z_HERE || s == Z_SEEN + Y_HERE => break,
                    _ => {}
                }
            }
        }
        if let Some(p) = found {
            // §609: reuse, and restrict the entries between q and p
            let info = stack[p].info;
            stack[q].info = info;
            let (code, from, to) = if info == Y_HERE {
                (o + (op::Y0 - op::DOWN1), YZ_OK, (Z_OK, Y_OK))
            } else {
                (o + (op::Z0 - op::DOWN1), YZ_OK, (Y_OK, Z_OK))
            };
            for e in &mut stack[p + 1..q] {
                if e.info == from {
                    e.info = to.0;
                } else if e.info == to.1 {
                    e.info = D_FIXED;
                }
            }
            self.dvi_out(code);
            return;
        }
        // §610
        stack[q].info = YZ_OK;
        if w.abs() >= 0o40000000 {
            self.dvi_out(o + 3);
            self.dvi_four(w);
            return;
        }
        let bytes = w.to_be_bytes();
        if w.abs() >= 0o100000 {
            self.dvi_out(o + 2);
            self.dvi_out(bytes[1]);
            self.dvi_out(bytes[2]);
        } else if w.abs() >= 0o200 {
            self.dvi_out(o + 1);
            self.dvi_out(bytes[2]);
        } else {
            self.dvi_out(o);
        }
        self.dvi_out(bytes[3]);
    }

    /// `prune_movements(l)` (§615): drop the stack entries at or past `l`.
    pub fn prune_movements(&mut self, l: usize) {
        while self.down.last().is_some_and(|e| e.location >= l) {
            self.down.pop();
        }
        while self.right.last().is_some_and(|e| e.location >= l) {
            self.right.pop();
        }
    }

    /// `push` for an opened box (§619), returning `save_loc`.
    fn enter(&mut self) -> usize {
        self.cur_s += 1;
        if self.cur_s > 0 {
            self.dvi_out(op::PUSH);
            self.op(Op::Push);
        }
        self.max_push = self.max_push.max(self.cur_s);
        self.location()
    }

    /// The end of `hlist_out`/`vlist_out` (§619, §629).
    fn leave(&mut self, save_loc: usize) {
        self.prune_movements(save_loc);
        if self.cur_s > 0 {
            self.dvi_pop(save_loc);
        }
        self.cur_s -= 1;
    }

    /// `synch_h` (§616).
    fn synch_h(&mut self) {
        if self.cur_h != self.dvi_h {
            self.movement(self.cur_h - self.dvi_h, Dir::Right);
            self.dvi_h = self.cur_h;
        }
    }

    /// `synch_v` (§616).
    fn synch_v(&mut self) {
        if self.cur_v != self.dvi_v {
            self.movement(self.cur_v - self.dvi_v, Dir::Down);
            self.dvi_v = self.cur_v;
        }
    }

    /// Outputs a box whose reference point is `(cur_h, cur_v)`.
    fn box_out(&mut self, b: &BoxNode, fonts: &dyn Fn(FontId) -> FontDef, wh: &mut Whatsits<'_>) {
        match b.kind {
            BoxKind::H => self.hlist_out(b, fonts, wh),
            BoxKind::V => self.vlist_out(b, fonts, wh),
        }
    }

    /// `special_out` (§1368) for a whatsit the caller says is a special.
    fn out_what(&mut self, h: u32, wh: &mut Whatsits<'_>) {
        // §1366
        if let Some(s) = wh(h, self.doing_leaders) {
            self.synch_h();
            self.synch_v();
            if let Some(t) = self.tap.0.as_mut() {
                tap_special(t.as_mut(), &s);
            }
            if self.ops.is_some() {
                self.op(Op::Special(s.clone()));
            }
            if s.len() < 256 {
                self.dvi_out(op::XXX1);
                self.dvi_out(low(u32::try_from(s.len()).unwrap_or(0)));
            } else {
                self.dvi_out(op::XXX4);
                self.dvi_four(i32::try_from(s.len()).unwrap_or(i32::MAX));
            }
            for b in s {
                self.dvi_out(b);
            }
        }
    }

    /// One character in an hlist (§620–§621); `lig`: the characters a
    /// ligature replaces, for the tap.
    fn char_out(&mut self, f: FontId, c: u16, lig: &[u16], fonts: &dyn Fn(FontId) -> FontDef) {
        self.synch_h();
        self.synch_v();
        self.set_font(f, fonts);
        // §620 (web2c: a character that doesn't exist is not output)
        let width = self.fonts[&f].widths.get(usize::from(c)).copied().flatten();
        if let Some(w) = width {
            if let Some(t) = self.tap.0.as_mut() {
                let size = self.fonts[&f].size;
                let (h, v) = (self.cur_h + ONE_TRUE_INCH, self.cur_v + ONE_TRUE_INCH);
                t.code((f, c), lig, true, f, low(u32::from(c)), h, v, w, size);
            }
            let c = low(u32::from(c));
            self.op(Op::Set(i32::from(c)));
            if c >= 128 {
                self.dvi_out(op::SET1);
            }
            self.dvi_out(c);
            self.cur_h += w;
        }
        self.dvi_h = self.cur_h;
    }

    /// A native word or glyph in an hlist (XeTeX's "Output the whatsit
    /// node `p` in an hlist": `set_glyphs`, or `set_text_and_glyphs`
    /// with the word's text).
    fn native_out(&mut self, n: &Native, fonts: &dyn Fn(FontId) -> FontDef) {
        self.synch_h();
        self.synch_v();
        self.set_font(n.font, fonts);
        if self.tap.0.is_some() {
            self.tap_native(n);
        }
        if self.ops.is_some() {
            let gs = |n: &Native| -> (Vec<(i32, i32)>, Vec<u16>) {
                (
                    n.glyphs.iter().map(|g| (g.x, g.y)).collect(),
                    n.glyphs.iter().map(|g| g.gid).collect(),
                )
            };
            let o = match n.kind {
                NativeKind::Glyph => Some(Op::Glyphs {
                    text: None,
                    width: n.width,
                    xy: vec![(0, 0)],
                    ids: vec![n.glyphs.first().map_or(0, |g| g.gid)],
                }),
                NativeKind::WordText if !n.text.is_empty() || !n.glyphs.is_empty() => {
                    let (xy, ids) = gs(n);
                    Some(Op::Glyphs {
                        text: Some(n.text.to_vec()),
                        width: n.width,
                        xy,
                        ids,
                    })
                }
                NativeKind::Word if !n.glyphs.is_empty() => {
                    let (xy, ids) = gs(n);
                    Some(Op::Glyphs {
                        text: None,
                        width: n.width,
                        xy,
                        ids,
                    })
                }
                _ => None,
            };
            if let Some(o) = o {
                self.op(o);
            }
        }
        match n.kind {
            NativeKind::Glyph => {
                self.dvi_out(op::SET_GLYPHS);
                self.dvi_four(n.width);
                self.dvi_two(1);
                self.dvi_four(0);
                self.dvi_four(0);
                self.dvi_two(n.glyphs.first().map_or(0, |g| g.gid));
            }
            NativeKind::WordText => {
                if !n.text.is_empty() || !n.glyphs.is_empty() {
                    self.dvi_out(op::SET_TEXT_AND_GLYPHS);
                    self.dvi_two(u16::try_from(n.text.len()).unwrap_or(u16::MAX));
                    for &u in n.text.iter() {
                        self.dvi_two(u);
                    }
                    self.glyph_array(n);
                }
            }
            NativeKind::Word => {
                if !n.glyphs.is_empty() {
                    self.dvi_out(op::SET_GLYPHS);
                    self.glyph_array(n);
                }
            }
        }
        self.cur_h += n.width;
        self.dvi_h = self.cur_h;
    }

    /// Native word or glyph `n`, at the current position, told the tap:
    /// the glyphs the file shows (a word's, or the glyph's one).
    fn tap_native(&mut self, n: &Native) {
        let (h, v) = (self.cur_h + ONE_TRUE_INCH, self.cur_v + ONE_TRUE_INCH);
        let size = self.fonts.get(&n.font).map_or(0, |d| d.size);
        let (text, gs): (&[u16], Vec<(Scaled, Scaled, Scaled)>) = match n.kind {
            NativeKind::Glyph => (&[], vec![(h, v, n.width)]),
            NativeKind::Word | NativeKind::WordText => (
                &n.text,
                n.glyphs
                    .iter()
                    .enumerate()
                    .map(|(i, g)| {
                        let next = n.glyphs.get(i + 1).map_or(n.width, |x| x.x);
                        (h + g.x, v, next - g.x)
                    })
                    .collect(),
            ),
        };
        if let Some(t) = self.tap.0.as_mut() {
            t.native(n.font, text, &gs, size);
        }
    }

    /// `make_xdv_glyph_array_data`: the width, the count, each glyph's x
    /// and y, then each glyph's id.
    fn glyph_array(&mut self, n: &Native) {
        self.dvi_four(n.width);
        self.dvi_two(u16::try_from(n.glyphs.len()).unwrap_or(u16::MAX));
        for g in n.glyphs.iter() {
            self.dvi_four(g.x);
            self.dvi_four(g.y);
        }
        for g in n.glyphs.iter() {
            self.dvi_two(g.gid);
        }
    }

    /// `dvi_two` (XeTeX): two bytes.
    fn dvi_two(&mut self, s: u16) {
        let [a, b] = s.to_be_bytes();
        self.dvi_out(a);
        self.dvi_out(b);
    }

    /// The font `f` selected (§621), defined at its first use.
    fn set_font(&mut self, f: FontId, fonts: &dyn Fn(FontId) -> FontDef) {
        if self.dvi_f != Some(f) {
            if let Some(r) = &mut self.rec
                && !r.used.contains(&f)
            {
                r.used.push(f);
                if !self.fonts.contains_key(&f) {
                    r.new.push(f);
                }
            }
            // §621
            if let std::collections::btree_map::Entry::Vacant(e) = self.fonts.entry(f) {
                e.insert(fonts(f));
                self.dvi_font_def(f);
            }
            let id = f;
            let f = self.num(id);
            let n = f - 1;
            self.op(Op::Font(n.cast_signed()));
            if f <= 64 {
                self.dvi_out(op::FNT_NUM_0 + low(n));
            } else if f <= 256 {
                self.dvi_out(op::FNT1);
                self.dvi_out(low(n));
            } else {
                self.dvi_out(op::FNT1 + 1);
                self.dvi_out(low(n / 0o400));
                self.dvi_out(low(n));
            }
            self.dvi_f = Some(id);
        }
    }

    /// `hlist_out` (§619).
    fn hlist_out(&mut self, b: &BoxNode, fonts: &dyn Fn(FontId) -> FontDef, wh: &mut Whatsits<'_>) {
        let save_loc = self.enter();
        let mut fr = Frame::new(b, self.cur_v, self.cur_h);
        // (e-TeX: a list without text direction nodes, outside
        // right-to-left text, comes out as TeX's)
        if self.cur_dir == 0
            && !b.list.iter().any(|n| matches!(n, Node::Math(k, _) if k.is_lr()))
        {
            self.hlist_nodes(&b.list, &mut fr, fonts, wh);
        } else {
            self.hlist_lr(&b.list, &mut fr, fonts, wh);
        }
        self.leave(save_loc);
    }

    /// `hlist_out` for mixed direction text (e-TeX's TeX--XeT, XeTeX
    /// §1409ff): right-to-left segments reversed as the output reaches
    /// them, between edges.
    fn hlist_lr(
        &mut self,
        list: &[Node],
        fr: &mut Frame,
        fonts: &dyn Fn(FontId) -> FontDef,
        wh: &mut Whatsits<'_>,
    ) {
        let mut q: std::collections::VecDeque<Lr> = std::collections::VecDeque::new();
        flatten_into(list, &mut q);
        // "Initialize the LR stack": `before`, never matched
        let mut lr: Vec<u8> = vec![0];
        if self.cur_dir == 1 {
            // "Reverse the complete hlist"
            let save_h = self.cur_h;
            self.cur_h = 0;
            let mut rev = self.reverse(fr, &mut q, None, &mut lr, fonts);
            rev.push_front(Lr::N(Node::Kern(-self.cur_h, KernKind::Explicit)));
            q = rev;
            self.cur_h = save_h;
        }
        fr.left_edge = self.cur_h;
        while let Some(it) = q.pop_front() {
            match it {
                Lr::Edge { dir, width, dist } => {
                    self.cur_h += width;
                    fr.left_edge = self.cur_h + dist;
                    self.cur_dir = dir;
                }
                Lr::Fixed(g) => {
                    let w = g.spec.width;
                    if let Some(rule) = self.hglue_wd(&g, w, fr, fonts, wh) {
                        self.hrule(rule, fr);
                    }
                }
                Lr::N(Node::Math(k, w)) => {
                    // "Adjust the LR stack for the `hlist_out` routine"
                    if k.is_end() {
                        if lr.last() == Some(&k.end_type()) {
                            lr.pop();
                        }
                    } else {
                        lr.push(k.end_type());
                        if k.dir() != self.cur_dir {
                            // "Reverse an hlist segment"
                            let save_h = self.cur_h;
                            self.cur_dir = 1 - self.cur_dir;
                            let dir = self.cur_dir;
                            self.cur_h = self.cur_h - fr.left_edge + w;
                            let tail = Lr::Edge {
                                dir: 1 - dir,
                                width: 0,
                                dist: 0,
                            };
                            let mut rev = self.reverse(fr, &mut q, Some(tail), &mut lr, fonts);
                            let dist = self.cur_h;
                            self.cur_dir = 1 - self.cur_dir;
                            self.cur_h = save_h;
                            rev.append(&mut q);
                            rev.push_front(Lr::Edge {
                                dir,
                                width: w,
                                dist,
                            });
                            q = rev;
                            continue;
                        }
                    }
                    self.cur_h += w;
                }
                Lr::N(n) => self.hlist_nodes(std::slice::from_ref(&n), fr, fonts, wh),
            }
        }
    }

    /// e-TeX's `reverse`: the nodes of `q` up to the end of the segment
    /// (all of them when `t` is `None`) in reverse order, `t` last, with
    /// glue set as it is output, `cur_h` advanced by their widths.
    #[allow(clippy::too_many_lines)]
    fn reverse(
        &mut self,
        fr: &mut Frame,
        q: &mut std::collections::VecDeque<Lr>,
        mut t: Option<Lr>,
        lr: &mut Vec<u8>,
        fonts: &dyn Fn(FontId) -> FontDef,
    ) -> std::collections::VecDeque<Lr> {
        let mut l: std::collections::VecDeque<Lr> = std::collections::VecDeque::new();
        let mut m = 0u32;
        let mut n = 0u32;
        loop {
            while let Some(it) = q.pop_front() {
                let mut node = match it {
                    Lr::N(node) => node,
                    Lr::Fixed(g) => {
                        self.cur_h += g.spec.width;
                        l.push_front(Lr::Fixed(g));
                        continue;
                    }
                    other @ Lr::Edge { .. } => {
                        l.push_front(other);
                        continue;
                    }
                };
                let rule_wd = match &mut node {
                    Node::Char { font, ch } => self.char_width(*font, *ch, fonts),
                    Node::Lig(lg) => {
                        let w = self.char_width(lg.font, lg.ch, fonts);
                        node = Node::Char {
                            font: lg.font,
                            ch: lg.ch,
                        };
                        w
                    }
                    Node::Box(bx) => bx.width,
                    Node::Rule(r) => r.width,
                    Node::Kern(w, _) => *w,
                    Node::Native(nw) => nw.width,
                    Node::Whatsit(_, Some(sz)) => sz.width,
                    Node::Glue(g) => {
                        let w = fr.glue(g);
                        // "Handle a glue node for mixed direction"
                        let set = match fr.g_sign {
                            GlueSign::Stretching => g.spec.stretch_order == fr.g_order,
                            GlueSign::Shrinking => g.spec.shrink_order == fr.g_order,
                            GlueSign::Normal => false,
                        };
                        if set {
                            if matches!(
                                g.kind,
                                GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
                            ) {
                                let mut g = g.clone();
                                g.spec.width = w;
                                self.cur_h += w;
                                l.push_front(Lr::Fixed(g));
                                continue;
                            }
                            node = Node::Kern(w, KernKind::Normal);
                        }
                        w
                    }
                    Node::Math(k, w) => {
                        let (k, w) = (*k, *w);
                        if k.is_end() {
                            if lr.last() == Some(&k.end_type()) {
                                lr.pop();
                                if n > 0 {
                                    n -= 1;
                                    node = Node::Math(k.flipped(), w);
                                } else {
                                    node = Node::Kern(w, KernKind::Normal);
                                    if m > 0 {
                                        m -= 1;
                                    } else {
                                        // "Finish the reversed hlist segment"
                                        let cur_h = self.cur_h;
                                        if let Some(Lr::Edge { dir, .. }) = t {
                                            l.push_back(Lr::Edge {
                                                dir,
                                                width: w,
                                                dist: -cur_h - w,
                                            });
                                        }
                                        return l;
                                    }
                                }
                            } else {
                                node = Node::Kern(w, KernKind::Normal);
                            }
                        } else {
                            lr.push(k.end_type());
                            if n > 0 || k.dir() != self.cur_dir {
                                n += 1;
                                node = Node::Math(k.flipped(), w);
                            } else {
                                node = Node::Kern(w, KernKind::Normal);
                                m += 1;
                            }
                        }
                        w
                    }
                    _ => {
                        l.push_front(Lr::N(node));
                        continue;
                    }
                };
                self.cur_h += rule_wd;
                if matches!(node, Node::Kern(..)) && (rule_wd == 0 || (l.is_empty() && t.is_none())) {
                    continue;
                }
                l.push_front(Lr::N(node));
            }
            if t.is_none() && m == 0 && n == 0 {
                break;
            }
            // "manufacture one missing math node"
            let top = lr.last().copied().unwrap_or(0);
            q.push_back(Lr::N(Node::Math(MathKind::of_subtype(i64::from(top)), 0)));
            if top == 0 {
                // (an empty stack: nothing left to close)
                q.clear();
                break;
            }
        }
        if let Some(t) = t.take() {
            l.push_back(t);
        }
        l
    }

    /// `char_width(f)(char_info(f)(c))`: 0 for a character the font
    /// does not have.
    fn char_width(&self, f: FontId, c: u16, fonts: &dyn Fn(FontId) -> FontDef) -> Scaled {
        let w = |d: &FontDef| d.widths.get(usize::from(c)).copied().flatten().unwrap_or(0);
        match self.fonts.get(&f) {
            Some(d) => w(d),
            None => w(&fonts(f)),
        }
    }

    /// The nodes of an hlist (§620, §622); a discretionary's replacement
    /// text is part of the list (tex.web's `replace_count` nodes).
    #[allow(clippy::too_many_lines)]
    fn hlist_nodes(
        &mut self,
        list: &[Node],
        fr: &mut Frame,
        fonts: &dyn Fn(FontId) -> FontDef,
        wh: &mut Whatsits<'_>,
    ) {
        for p in list {
            let rule = match p {
                Node::Char { font, ch } => {
                    self.char_out(*font, *ch, &[], fonts);
                    None
                }
                // §652: a ligature is output as its character
                Node::Lig(l) => {
                    self.char_out(l.font, l.ch, &l.original, fonts);
                    None
                }
                Node::Box(bx) => {
                    // §623
                    if bx.list.is_empty() {
                        self.cur_h += bx.width;
                    } else {
                        let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                        self.cur_v = fr.base + bx.shift;
                        let edge = self.cur_h + bx.width;
                        if self.cur_dir == 1 {
                            self.cur_h = edge;
                        }
                        self.box_out(bx, fonts, wh);
                        self.dvi_h = save_h;
                        self.dvi_v = save_v;
                        self.cur_h = edge;
                        self.cur_v = fr.base;
                    }
                    None
                }
                Node::Rule(r) => Some((r.height, r.depth, r.width)),
                // (XeTeX's picture: its special at the baseline, the
                // width passed over)
                Node::Whatsit(h, Some(sz)) if self.xdv => {
                    let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                    self.cur_v = fr.base;
                    let edge = self.cur_h + sz.width;
                    self.out_what(*h, wh);
                    self.dvi_h = save_h;
                    self.dvi_v = save_v;
                    self.cur_h = edge;
                    self.cur_v = fr.base;
                    None
                }
                // a sized whatsit is pdfTeX's, refused in DVI mode
                Node::Whatsit(h, _) => {
                    self.out_what(*h, wh);
                    None
                }
                Node::Native(n) => {
                    self.native_out(n, fonts);
                    None
                }
                Node::Glue(g) => self.hglue(g, fr, fonts, wh),
                Node::Kern(w, _) | Node::Math(_, w) => {
                    self.cur_h += w;
                    None
                }
                Node::Disc(d) => {
                    self.hlist_nodes(&d.replace, fr, fonts, wh);
                    None
                }
                Node::Unset(_)
                | Node::Ins(_)
                | Node::Mark(_)
                | Node::Adjust(_)
                | Node::Language { .. }
                | Node::Penalty(_) => None,
            };
            if let Some(rule) = rule {
                self.hrule(rule, fr);
            }
        }
    }

    /// A rule in an hlist (§624), then past it (§622 `move_past`).
    fn hrule(&mut self, (mut rule_ht, mut rule_dp, rule_wd): (Scaled, Scaled, Scaled), fr: &Frame) {
        if rule_ht == RUNNING {
            rule_ht = fr.height;
        }
        if rule_dp == RUNNING {
            rule_dp = fr.depth;
        }
        rule_ht += rule_dp;
        if rule_ht > 0 && rule_wd > 0 {
            self.synch_h();
            self.cur_v = fr.base + rule_dp;
            self.synch_v();
            self.op(Op::SetRule {
                height: rule_ht,
                width: rule_wd,
            });
            self.dvi_out(op::SET_RULE);
            self.dvi_four(rule_ht);
            self.dvi_four(rule_wd);
            self.cur_v = fr.base;
            self.dvi_h += rule_wd;
        }
        self.cur_h += rule_wd;
    }

    /// Glue in an hlist (§625–§628): the rule to output (`fin_rule`), or
    /// `None` once `cur_h` has moved past it.
    fn hglue(
        &mut self,
        g: &GlueNode,
        fr: &mut Frame,
        fonts: &dyn Fn(FontId) -> FontDef,
        wh: &mut Whatsits<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        let rule_wd = fr.glue(g);
        self.hglue_wd(g, rule_wd, fr, fonts, wh)
    }

    /// Glue in an hlist set to `rule_wd` (§626–§628, with e-TeX's
    /// right-to-left leaders).
    fn hglue_wd(
        &mut self,
        g: &GlueNode,
        mut rule_wd: Scaled,
        fr: &mut Frame,
        fonts: &dyn Fn(FontId) -> FontDef,
        wh: &mut Whatsits<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        if matches!(
            g.kind,
            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
        ) {
            // §626
            match g.leader.as_deref() {
                Some(Node::Rule(r)) => return Some((r.height, r.depth, rule_wd)),
                Some(Node::Box(lb)) => {
                    let leader_wd = lb.width;
                    if leader_wd > 0 && rule_wd > 0 {
                        rule_wd += 10;
                        if self.cur_dir == 1 {
                            self.cur_h -= 10;
                        }
                        let edge = self.cur_h + rule_wd;
                        let mut lx = 0;
                        // §627
                        if g.kind == GlueKind::ALeaders {
                            let save_h = self.cur_h;
                            self.cur_h = fr.left_edge
                                + leader_wd * ((self.cur_h - fr.left_edge) / leader_wd);
                            if self.cur_h < save_h {
                                self.cur_h += leader_wd;
                            }
                        } else {
                            let lq = rule_wd / leader_wd;
                            let lr = rule_wd % leader_wd;
                            if g.kind == GlueKind::CLeaders {
                                self.cur_h += lr / 2;
                            } else {
                                lx = lr / (lq + 1);
                                self.cur_h += (lr - (lq - 1) * lx) / 2;
                            }
                        }
                        while self.cur_h + leader_wd <= edge {
                            // §628
                            self.cur_v = fr.base + lb.shift;
                            self.synch_v();
                            let save_v = self.dvi_v;
                            self.synch_h();
                            let save_h = self.dvi_h;
                            if self.cur_dir == 1 {
                                self.cur_h += leader_wd;
                            }
                            let outer = self.doing_leaders;
                            self.doing_leaders = true;
                            self.box_out(lb, fonts, wh);
                            self.doing_leaders = outer;
                            self.dvi_v = save_v;
                            self.dvi_h = save_h;
                            self.cur_v = fr.base;
                            self.cur_h = save_h + leader_wd + lx;
                        }
                        self.cur_h = if self.cur_dir == 1 { edge } else { edge - 10 };
                        return None;
                    }
                }
                _ => {}
            }
        }
        // §622 `move_past`
        self.cur_h += rule_wd;
        None
    }

    /// `vlist_out` (§629).
    fn vlist_out(&mut self, b: &BoxNode, fonts: &dyn Fn(FontId) -> FontDef, wh: &mut Whatsits<'_>) {
        let save_loc = self.enter();
        let left_edge = self.cur_h;
        self.cur_v -= b.height;
        let mut fr = Frame::new(b, self.cur_v, left_edge);
        for p in &b.list {
            // §630–§631
            let rule = match p {
                Node::Char { .. } | Node::Lig(_) => panic!("vlistout"),
                Node::Box(bx) => {
                    // §632
                    if bx.list.is_empty() {
                        self.cur_v += bx.height + bx.depth;
                    } else {
                        self.cur_v += bx.height;
                        self.synch_v();
                        let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                        self.cur_h = if self.cur_dir == 1 {
                            left_edge - bx.shift
                        } else {
                            left_edge + bx.shift
                        };
                        self.box_out(bx, fonts, wh);
                        self.dvi_h = save_h;
                        self.dvi_v = save_v;
                        self.cur_v = save_v + bx.depth;
                        self.cur_h = left_edge;
                    }
                    None
                }
                Node::Rule(r) => Some((r.height, r.depth, r.width)),
                // (XeTeX's picture: its special at its top, then below
                // it from where the output stood)
                Node::Whatsit(h, Some(sz)) if self.xdv => {
                    let (save_h, save_v) = (self.dvi_h, self.dvi_v);
                    self.cur_v += sz.height;
                    self.out_what(*h, wh);
                    self.dvi_h = save_h;
                    self.dvi_v = save_v;
                    self.cur_v = save_v + sz.depth;
                    self.cur_h = left_edge;
                    None
                }
                // a sized whatsit is pdfTeX's, refused in DVI mode
                Node::Whatsit(h, _) => {
                    self.out_what(*h, wh);
                    None
                }
                Node::Native(n) if n.kind == NativeKind::Glyph => {
                    // XeTeX's "Output the whatsit node `p` in a vlist"
                    self.cur_v += n.height;
                    self.cur_h = left_edge;
                    self.synch_h();
                    self.synch_v();
                    self.set_font(n.font, fonts);
                    if self.tap.0.is_some() {
                        self.tap_native(n);
                    }
                    self.op(Op::Glyphs {
                        text: None,
                        width: 0,
                        xy: vec![(0, 0)],
                        ids: vec![n.glyphs.first().map_or(0, |g| g.gid)],
                    });
                    self.dvi_out(op::SET_GLYPHS);
                    self.dvi_four(0);
                    self.dvi_two(1);
                    self.dvi_four(0);
                    self.dvi_four(0);
                    self.dvi_two(n.glyphs.first().map_or(0, |g| g.gid));
                    self.cur_v += n.depth;
                    self.cur_h = left_edge;
                    None
                }
                Node::Glue(g) => self.vglue(g, &mut fr, fonts, wh),
                Node::Kern(w, _) => {
                    self.cur_v += w;
                    None
                }
                _ => None,
            };
            if let Some((rule_ht, rule_dp, mut rule_wd)) = rule {
                // §633
                if rule_wd == RUNNING {
                    rule_wd = fr.width;
                }
                let rule_ht = rule_ht + rule_dp;
                self.cur_v += rule_ht;
                if rule_ht > 0 && rule_wd > 0 {
                    if self.cur_dir == 1 {
                        self.cur_h -= rule_wd;
                    }
                    self.synch_h();
                    self.synch_v();
                    self.op(Op::PutRule {
                        height: rule_ht,
                        width: rule_wd,
                    });
                    self.dvi_out(op::PUT_RULE);
                    self.dvi_four(rule_ht);
                    self.dvi_four(rule_wd);
                    self.cur_h = left_edge;
                }
            }
        }
        self.leave(save_loc);
    }

    /// Glue in a vlist (§634–§637): the rule to output (`fin_rule`), or
    /// `None` once `cur_v` has moved past it.
    fn vglue(
        &mut self,
        g: &GlueNode,
        fr: &mut Frame,
        fonts: &dyn Fn(FontId) -> FontDef,
        wh: &mut Whatsits<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        let mut rule_ht = fr.glue(g);
        if matches!(
            g.kind,
            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
        ) {
            // §635
            match g.leader.as_deref() {
                Some(Node::Rule(r)) => return Some((rule_ht, 0, r.width)),
                Some(Node::Box(lb)) => {
                    let leader_ht = lb.height + lb.depth;
                    if leader_ht > 0 && rule_ht > 0 {
                        rule_ht += 10;
                        let edge = self.cur_v + rule_ht;
                        let mut lx = 0;
                        // §636
                        if g.kind == GlueKind::ALeaders {
                            let save_v = self.cur_v;
                            self.cur_v = fr.base + leader_ht * ((self.cur_v - fr.base) / leader_ht);
                            if self.cur_v < save_v {
                                self.cur_v += leader_ht;
                            }
                        } else {
                            let lq = rule_ht / leader_ht;
                            let lr = rule_ht % leader_ht;
                            if g.kind == GlueKind::CLeaders {
                                self.cur_v += lr / 2;
                            } else {
                                lx = lr / (lq + 1);
                                self.cur_v += (lr - (lq - 1) * lx) / 2;
                            }
                        }
                        while self.cur_v + leader_ht <= edge {
                            // §637
                            self.cur_h = if self.cur_dir == 1 {
                                fr.left_edge - lb.shift
                            } else {
                                fr.left_edge + lb.shift
                            };
                            self.synch_h();
                            let save_h = self.dvi_h;
                            self.cur_v += lb.height;
                            self.synch_v();
                            let save_v = self.dvi_v;
                            let outer = self.doing_leaders;
                            self.doing_leaders = true;
                            self.box_out(lb, fonts, wh);
                            self.doing_leaders = outer;
                            self.dvi_v = save_v;
                            self.dvi_h = save_h;
                            self.cur_h = fr.left_edge;
                            self.cur_v = save_v - lb.height + leader_ht + lx;
                        }
                        self.cur_v = edge - 10;
                        return None;
                    }
                }
                _ => {}
            }
        }
        // §631 `move_past`
        self.cur_v += rule_ht;
        None
    }

    /// `ship_out(p)` (§638, §640–§641): one page with `\count0..9`.
    /// `fonts` answers each font's definition the first time it is used
    /// (font ids are TeX's internal font numbers, `null_font` being 0,
    /// so font `f` is DVI font `f-1`); `whatsits` answers specials.
    ///
    /// # Errors
    /// [`ShipError::HugePage`] for a page TeX refuses (§641); nothing is
    /// written then.
    pub fn ship_out(
        &mut self,
        page: &BoxNode,
        counts: [i32; 10],
        fonts: &dyn Fn(FontId) -> FontDef,
        whatsits: &mut Whatsits<'_>,
    ) -> Result<(), ShipError> {
        // §641
        let too_big = |a: Scaled, b: Scaled| a.checked_add(b).is_none_or(|s| s > MAX_DIMEN);
        if page.height > MAX_DIMEN
            || page.depth > MAX_DIMEN
            || page
                .height
                .checked_add(page.depth)
                .is_none_or(|hd| too_big(hd, self.v_offset))
            || too_big(page.width, self.h_offset)
        {
            return Err(ShipError::HugePage);
        }
        self.max_v = self.max_v.max(page.height + page.depth + self.v_offset);
        self.max_h = self.max_h.max(page.width + self.h_offset);
        // §617 (the preamble went out in `new`)
        self.dvi_h = 0;
        self.dvi_v = 0;
        self.cur_h = self.h_offset;
        self.dvi_f = None;
        // §640
        let page_loc = i32::try_from(self.location()).unwrap_or(i32::MAX);
        self.dvi_out(op::BOP);
        for c in counts {
            self.dvi_four(c);
        }
        self.dvi_four(self.last_bop);
        self.last_bop = page_loc;
        if self.xdv {
            // XeTeX: "generate a pagesize special at start of page"
            let mut s = String::from("pdf:pagesize ");
            let (w, h) = self.page_size;
            if w > 0 && h > 0 {
                s.push_str("width ");
                print_scaled(&mut s, w);
                s.push_str("pt height ");
                print_scaled(&mut s, h);
                s.push_str("pt");
            } else {
                s.push_str("default");
            }
            self.op(Op::Special(s.as_bytes().to_vec()));
            self.dvi_out(op::XXX1);
            self.dvi_out(low(u32::try_from(s.len()).unwrap_or(0)));
            for &b in s.as_bytes() {
                self.dvi_out(b);
            }
        }
        self.cur_v = page.height + self.v_offset;
        self.box_out(page, fonts, whatsits);
        self.dvi_out(op::EOP);
        self.total_pages += 1;
        self.cur_s = -1;
        Ok(())
    }

    /// Font `f` as defined before the next page, without its `fnt_def`:
    /// a page written apart from the pages before it
    /// ([`DviWriter::ship_out_kept`], then [`DviWriter::ship_kept`] in
    /// the file, which checks the fonts it finds defined there).
    pub fn assume_defined(&mut self, f: FontId, d: FontDef) {
        self.fonts.insert(f, d);
    }

    /// The buffer's state at the next byte.
    fn phase(&self) -> Phase {
        Phase {
            ptr: self.dvi_ptr,
            limit: self.dvi_limit,
            back: self.location() - self.dvi_gone,
        }
    }

    /// [`DviWriter::ship_out`], keeping the page's bytes and what they
    /// depend on for [`DviWriter::ship_kept`].
    ///
    /// # Errors
    /// As [`DviWriter::ship_out`].
    pub fn ship_out_kept(
        &mut self,
        page: &BoxNode,
        counts: [i32; 10],
        fonts: &dyn Fn(FontId) -> FontDef,
        whatsits: &mut Whatsits<'_>,
    ) -> Result<PageBytes, ShipError> {
        let start = self.location();
        let phase = self.phase();
        let max_push = std::mem::take(&mut self.max_push);
        self.rec = Some(Recording {
            start,
            tape: Vec::new(),
            used: Vec::new(),
            new: Vec::new(),
        });
        let r = self.ship_out(page, counts, fonts, whatsits);
        let rec = self.rec.take();
        let page_push = self.max_push;
        self.max_push = max_push.max(page_push);
        r?;
        let rec = rec.unwrap_or(Recording {
            start,
            tape: Vec::new(),
            used: Vec::new(),
            new: Vec::new(),
        });
        let used = rec
            .used
            .iter()
            .map(|f| {
                let d = &self.fonts[f];
                (
                    *f,
                    d.checksum,
                    d.size,
                    d.design_size,
                    d.area.clone(),
                    d.name.clone(),
                )
            })
            .collect();
        let new = rec
            .new
            .iter()
            .map(|f| (*f, self.fonts[f].clone()))
            .collect();
        Ok(PageBytes {
            bytes: self.out[start..].to_vec(),
            start: phase,
            end: self.phase(),
            tape: rec.tape,
            used,
            new,
            offsets: (self.h_offset, self.v_offset),
            max_v: page.height + page.depth + self.v_offset,
            max_h: page.width + self.h_offset,
            max_push: page_push,
        })
    }

    /// Ships a page [`DviWriter::ship_out_kept`] wrote earlier, its bytes
    /// as they were but for the pointer to the previous `bop` (§640),
    /// when they are what `ship_out` would write here: the same margins,
    /// the same fonts defined before it of those it uses, and every
    /// decision that reads the buffer's state (§601, §611) coming out as
    /// it did. False, and nothing written, otherwise.
    pub fn ship_kept(&mut self, p: &PageBytes) -> bool {
        if p.offsets != (self.h_offset, self.v_offset) || p.bytes.len() < 45 {
            return false;
        }
        for (f, ck, sz, ds, area, name) in &p.used {
            let new = p.new.iter().any(|(g, _)| g == f);
            match self.fonts.get(f) {
                None if new => {}
                Some(d)
                    if !new
                        && d.checksum == *ck
                        && d.size == *sz
                        && d.design_size == *ds
                        && d.area == *area
                        && d.name == *name => {}
                _ => return false,
            }
        }
        let phase = self.phase();
        let end = if phase == p.start {
            p.end
        } else {
            match replay(self.buf_size, phase, &p.tape, p.bytes.len()) {
                Some(e) => e,
                None => return false,
            }
        };
        let page_loc = self.location();
        self.out.extend_from_slice(&p.bytes);
        self.loc += p.bytes.len();
        let back = i32::try_from(page_loc).unwrap_or(i32::MAX);
        self.out[page_loc + 41..page_loc + 45].copy_from_slice(&self.last_bop.to_be_bytes());
        self.last_bop = back;
        let loc = self.location();
        self.dvi_ptr = end.ptr;
        self.dvi_limit = end.limit;
        self.dvi_offset = loc - end.ptr;
        self.dvi_gone = loc - end.back;
        for (f, d) in &p.new {
            self.fonts.insert(*f, d.clone());
        }
        self.max_v = self.max_v.max(p.max_v);
        self.max_h = self.max_h.max(p.max_h);
        self.max_push = self.max_push.max(p.max_push);
        self.total_pages += 1;
        self.cur_s = -1;
        true
    }

    /// The postamble (§642–§643); no file at all without pages.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        if self.total_pages == 0 {
            return Vec::new();
        }
        self.dvi_out(op::POST);
        self.dvi_four(self.last_bop);
        self.last_bop = i32::try_from(self.location() - 5).unwrap_or(i32::MAX);
        self.dvi_four(25400000);
        self.dvi_four(473628672);
        self.dvi_four(self.mag);
        self.dvi_four(self.max_v);
        self.dvi_four(self.max_h);
        let [_, _, a, b] = self.max_push.to_be_bytes();
        self.dvi_out(a);
        self.dvi_out(b);
        let [_, _, a, b] = self.total_pages.to_be_bytes();
        self.dvi_out(a);
        self.dvi_out(b);
        // §643: from the highest font number down
        let mut used: Vec<FontId> = self.fonts.keys().copied().collect();
        used.sort_unstable_by_key(|&f| std::cmp::Reverse(self.num(f)));
        for f in used {
            self.dvi_font_def(f);
        }
        self.dvi_out(op::POST_POST);
        self.dvi_four(self.last_bop);
        self.dvi_out(self.id_byte());
        let k = 4 + (self.buf_size - self.dvi_ptr) % 4;
        for _ in 0..k {
            self.dvi_out(223);
        }
        self.out
    }
}

/// `print_scaled` (§103).
fn print_scaled(out: &mut String, s: Scaled) {
    let mut s = i64::from(s);
    if s < 0 {
        out.push('-');
        s = -s;
    }
    out.push_str(&(s / 65536).to_string());
    out.push('.');
    s = 10 * (s % 65536) + 5;
    let mut delta = 10;
    loop {
        if delta > 65536 {
            s += 0o100000 - 50000;
        }
        out.push(char::from(b'0' + u8::try_from(s / 65536).unwrap_or(0)));
        s = 10 * (s % 65536);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
}

/// The low byte of `n`.
fn low(n: u32) -> u8 {
    n.to_be_bytes()[3]
}

/// A `\special` told the tap as dvipdfmx draws it: a picture or form
/// placed (`pdf:image`, `pdf:uxobj`, XeTeX's pictures among them), or
/// content put in the page's stream (`pdf:literal`, `pdf:content`,
/// `pdf:code`), whose text's codes the tap counts.
fn tap_special(t: &mut dyn GlyphTap, s: &[u8]) {
    let s = s.trim_ascii_start();
    let Some(rest) = s.strip_prefix(b"pdf:") else {
        return;
    };
    let word = rest
        .iter()
        .position(u8::is_ascii_whitespace)
        .map_or(rest, |k| &rest[..k]);
    match word {
        b"image" | b"uxobj" => t.xobject(),
        b"literal" | b"content" | b"code" => t.literal(&rest[word.len()..]),
        _ => {}
    }
}
