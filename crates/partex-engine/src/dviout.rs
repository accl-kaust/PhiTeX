//! The DVI backend: writes [`Page`]s as DVI (tex.web part 31, §583–§615,
//! and the output half of part 32, §616–§643).
//!
//! It owns every piece of DVI state (buffer, movement stacks, positions,
//! fonts already defined) and never looks at the engine. The half-buffer
//! scheme is kept exactly: `movement` only rewrites commands still in the
//! buffer (`dvi_gone`), and `dvi_pop` can only retract a `push` that is,
//! so the bytes depend on it.

use alloc::vec::Vec;

use crate::Scaled;
use core::ops::Range;

use crate::pageir::{FontDef, Item, LeaderKind, Page};

/// At most this many bytes per page item: a character, rule, box or
/// move with its font change and position synchronization (`push`,
/// `pop`, `fnt1`, `set1`, two 5-byte movements, a 9-byte rule…).
const ITEM_BOUND: u64 = 48;

/// An upper bound on what [`DviWriter::page`] writes for `page`; `None`
/// if the page has leaders (their repetitions depend on the glue).
#[must_use]
pub fn page_bound(page: &Page) -> Option<u64> {
    let mut n = 64 + (page.specials.len() + page.prelude.len() + page.native.len()) as u64; // `bop`, `eop`
    for f in &page.fonts {
        n += 16 + (f.area.len() + f.name.len() + f.native.as_ref().map_or(0, Vec::len)) as u64;
    }
    for item in &page.items {
        if let Item::Leaders { .. } = item {
            return None;
        }
        n += ITEM_BOUND;
    }
    Some(n)
}

/// §586: DVI opcodes.
const SET1: u8 = 128;
const SET_RULE: u8 = 132;
const PUT_RULE: u8 = 137;
const BOP: u8 = 139;
const EOP: u8 = 140;
const PUSH: u8 = 141;
const POP: u8 = 142;
const RIGHT1: u8 = 143;
const DOWN1: u8 = 157;
const Y0: u8 = 161;
const Y1: u8 = 162;
const Z0: u8 = 166;
const Z1: u8 = 167;
const FNT_NUM_0: u8 = 171;
const FNT1: u8 = 235;
const XXX1: u8 = 239;
const XXX4: u8 = 242;
const FNT_DEF1: u8 = 243;
const PRE: u8 = 247;
const POST: u8 = 248;
const POST_POST: u8 = 249;
const ID_BYTE: u8 = 2;
/// `XeTeX`: XDV's `id_byte`, and its commands.
const XDV_ID_BYTE: u8 = 7;
const DEFINE_NATIVE_FONT: u8 = 252;

/// §171 `font_base` (internal font numbers start after it).
const FONT_BASE: i32 = 0;
/// §232 `null_font`.
const NULL_FONT: i32 = FONT_BASE;

/// §608: what a movement stack entry allows.
const Y_HERE: u8 = 1;
const Z_HERE: u8 = 2;
const YZ_OK: u8 = 3;
const Y_OK: u8 = 4;
const Z_OK: u8 = 5;
const D_FIXED: u8 = 6;
/// §611: search states.
const NONE_SEEN: u8 = 0;
const Y_SEEN: u8 = 6;
const Z_SEEN: u8 = 12;

/// The file would exceed 2^31 bytes (§598's fatal error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLong;

/// §605: one entry of the `down_ptr`/`right_ptr` stacks.
#[derive(Clone, Copy, Debug, Hash)]
struct Movement {
    width: Scaled,
    location: i32,
    info: u8,
}

crate::persist_struct!(Movement {
    width,
    location,
    info
});

/// Totals for the "Output written" line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub pages: i32,
    pub bytes: i32,
}

#[derive(Clone)]
pub struct DviWriter {
    /// Bytes ready to be written to the file.
    out: Vec<u8>,
    buf: Vec<u8>,
    buf_size: i32,
    half_buf: i32,
    limit: i32,
    ptr: i32,
    offset: i32,
    gone: i32,
    total_pages: i32,
    max_v: Scaled,
    max_h: Scaled,
    max_push: i32,
    last_bop: i32,
    down: Vec<Movement>,
    right: Vec<Movement>,
    dvi_h: Scaled,
    dvi_v: Scaled,
    cur_h: Scaled,
    cur_v: Scaled,
    dvi_f: i32,
    /// e-TeX: the current text direction is right to left (`cur_dir`).
    rtl: bool,
    /// §616: the current depth of output box nesting; above -1 between
    /// pages only when a truncated page was left open.
    cur_s: i32,
    /// Font definitions by internal font number, with whether the font
    /// has been used (`font_used`, §549).
    fonts: Vec<Option<(FontDef, bool)>>,
    /// `XeTeX`'s XDV (its `id_byte`).
    xdv: bool,
}

crate::persist_struct!(DviWriter {
    out,
    buf,
    buf_size,
    half_buf,
    limit,
    ptr,
    offset,
    gone,
    total_pages,
    max_v,
    max_h,
    max_push,
    last_bop,
    down,
    right,
    dvi_h,
    dvi_v,
    cur_h,
    cur_v,
    dvi_f,
    rtl,
    cur_s,
    fonts,
    xdv
});

impl DviWriter {
    /// A writer with tex.web's `dvi_buf_size`, having written the preamble
    /// (§617) with magnification `mag` and `comment`.
    pub fn new(buf_size: i32, mag: i32, comment: &[u8]) -> Result<Self, TooLong> {
        Self::with_format(buf_size, mag, comment, false)
    }

    /// [`DviWriter::new`] of a DVI file, or of `XeTeX`'s XDV with `xdv`.
    pub fn with_format(
        buf_size: i32,
        mag: i32,
        comment: &[u8],
        xdv: bool,
    ) -> Result<Self, TooLong> {
        let mut w = Self {
            out: Vec::new(),
            buf: alloc::vec![0; usize::try_from(buf_size).unwrap_or(0) + 1],
            buf_size,
            half_buf: buf_size / 2,
            limit: buf_size,
            ptr: 0,
            offset: 0,
            gone: 0,
            total_pages: 0,
            max_v: 0,
            max_h: 0,
            max_push: 0,
            last_bop: -1,
            down: Vec::new(),
            right: Vec::new(),
            dvi_h: 0,
            dvi_v: 0,
            cur_h: 0,
            cur_v: 0,
            dvi_f: NULL_FONT,
            rtl: false,
            cur_s: -1,
            fonts: Vec::new(),
            xdv,
        };
        w.out_byte(PRE)?;
        w.out_byte(w.id_byte())?;
        w.four(25_400_000)?;
        w.four(473_628_672)?; // conversion ratio for sp
        w.four(mag)?;
        w.out_int(i32::try_from(comment.len()).unwrap_or(0))?;
        for &c in comment {
            w.out_byte(c)?;
        }
        Ok(w)
    }

    /// The `id_byte` of the file.
    fn id_byte(&self) -> u8 {
        if self.xdv { XDV_ID_BYTE } else { ID_BYTE }
    }

    /// Pages written so far.
    #[must_use]
    pub fn total_pages(&self) -> i32 {
        self.total_pages
    }

    /// The length of the file so far (written or still buffered).
    #[must_use]
    pub fn length(&self) -> i32 {
        self.offset + self.ptr
    }

    /// Hash what later pages depend on: not where the file is (a
    /// checkpoint session relocates that, taking the writer of the new
    /// run at the same page), only what it says.
    pub fn hash_state<H: core::hash::Hasher>(&self, h: &mut H) {
        use core::hash::Hash;
        (
            self.total_pages,
            self.max_v,
            self.max_h,
            self.max_push,
            self.cur_s,
        )
            .hash(h);
        (&self.down, &self.right, self.rtl).hash(h);
        self.fonts.hash(h);
    }

    /// Hash where the file is and the bytes not written to it yet, which
    /// later output carries: for a runtime that reuses a region's output
    /// as it is, so a state is the same only where the file is too. A
    /// page's `bop` points back at the one before by its offset (§640),
    /// and a movement is reused only while its bytes are in the buffer
    /// (§611, `gone`): a DVI page is not position-independent.
    pub fn hash_placement<H: core::hash::Hasher>(&self, h: &mut H) {
        use core::hash::Hash;
        (self.offset, self.ptr, self.limit, self.gone, self.last_bop).hash(h);
        self.out.hash(h);
        // (the bytes from `gone` on, in the buffer's order: the half not
        // written yet, then the one being filled)
        let pending = usize::try_from(self.offset + self.ptr - self.gone).unwrap_or(0);
        let ptr = usize::try_from(self.ptr).unwrap_or(0);
        if pending <= ptr {
            self.buf[ptr - pending..ptr].hash(h);
        } else {
            let size = usize::try_from(self.buf_size).unwrap_or(0);
            self.buf[size.saturating_sub(pending - ptr)..size].hash(h);
            self.buf[..ptr].hash(h);
        }
    }

    /// The version of part `part` of the writer's state, from its
    /// content (DESIGN 7.17.12's `dvi` row): 0 the fonts defined, 1 the
    /// totals (`total_pages`, `max_v`, `max_h`, `max_push`), 2 the rest:
    /// where the file is, the bytes not written yet, the movements the
    /// buffer still holds, the open levels.
    #[must_use]
    pub fn part_version(&self, part: usize) -> u128 {
        use core::hash::Hash;
        let mut h = crate::stablehash::StableHasher::new();
        match part {
            0 => self.fonts.hash(&mut h),
            1 => (self.total_pages, self.max_v, self.max_h, self.max_push).hash(&mut h),
            _ => {
                (self.cur_s, &self.down, &self.right, self.rtl).hash(&mut h);
                (self.dvi_h, self.dvi_v, self.cur_h, self.cur_v, self.dvi_f).hash(&mut h);
                (self.buf_size, self.half_buf).hash(&mut h);
                self.hash_placement(&mut h);
            }
        }
        h.finish128()
    }

    /// Take the bytes that are ready for the file.
    pub fn drain(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.out)
    }

    /// §640: write one page.
    pub fn page(&mut self, page: &Page) -> Result<(), TooLong> {
        let root = head(page, 0);
        // §641: update `max_h` and `max_v` (the engine has rejected huge
        // pages).
        self.max_v = self.max_v.max(root.height + root.depth + page.v_offset);
        self.max_h = self.max_h.max(root.width + page.h_offset);
        for f in &page.fonts {
            let i = ux(f.font);
            if self.fonts.len() <= i {
                self.fonts.resize(i + 1, None);
            }
            if self.fonts[i].is_none() {
                self.fonts[i] = Some((f.clone(), false));
            }
        }
        // §617: initialize variables as `ship_out` begins.
        self.dvi_h = 0;
        self.dvi_v = 0;
        self.cur_h = page.h_offset;
        self.dvi_f = NULL_FONT;
        self.rtl = false;
        let page_loc = self.offset + self.ptr;
        self.out_byte(BOP)?;
        for &c in &page.counts {
            self.four(c)?;
        }
        self.four(self.last_bop)?;
        self.last_bop = page_loc;
        if !page.prelude.is_empty() {
            // (`XeTeX`'s pagesize special, where `bop` leaves the position)
            self.out_byte(XXX1)?;
            self.out_int(i32::try_from(page.prelude.len()).unwrap_or(0))?;
            for &c in &page.prelude {
                self.out_byte(c)?;
            }
        }
        self.cur_v = root.height + page.v_offset;
        let complete = self.list_out(page, 0)?;
        if complete {
            self.out_byte(EOP)?;
            self.total_pages += 1;
            self.cur_s = -1;
        }
        Ok(())
    }

    /// §642: close a page left open and write the postamble, with the
    /// fonts used in decreasing order of internal number.
    pub fn finish(&mut self, mag: i32) -> Result<Summary, TooLong> {
        while self.cur_s > -1 {
            if self.cur_s > 0 {
                self.out_byte(POP)?;
            } else {
                self.out_byte(EOP)?;
                self.total_pages += 1;
            }
            self.cur_s -= 1;
        }
        self.out_byte(POST)?; // beginning of the postamble
        self.four(self.last_bop)?;
        self.last_bop = self.offset + self.ptr - 5; // `post` location
        self.four(25_400_000)?;
        self.four(473_628_672)?; // conversion ratio for sp
        self.four(mag)?;
        self.four(self.max_v)?;
        self.four(self.max_h)?;
        self.out_int(self.max_push / 256)?;
        self.out_int(self.max_push % 256)?;
        self.out_int((self.total_pages / 256) % 256)?;
        self.out_int(self.total_pages % 256)?;
        // §643: output the font definitions for all fonts that were used.
        let fonts = core::mem::take(&mut self.fonts);
        for (def, _) in fonts.iter().rev().flatten().filter(|(_, used)| *used) {
            self.font_def(def)?;
        }
        self.out_byte(POST_POST)?;
        self.four(self.last_bop)?;
        self.out_byte(self.id_byte())?;
        let mut k = 4 + ((self.buf_size - self.ptr) % 4); // the number of 223's
        while k > 0 {
            self.out_byte(223)?;
            k -= 1;
        }
        // §599: empty the last bytes out of `dvi_buf`.
        if self.limit == self.half_buf {
            self.write(self.half_buf, self.buf_size - 1);
        }
        if self.ptr > 0x7FFF_FFFF - self.offset {
            return Err(TooLong);
        }
        if self.ptr > 0 {
            self.write(0, self.ptr - 1);
        }
        Ok(Summary {
            pages: self.total_pages,
            bytes: self.offset + self.ptr,
        })
    }

    /// §597: `write_dvi(a,b)`.
    fn write(&mut self, a: i32, b: i32) {
        let (a, b) = (ux(a), ux(b));
        self.out.extend_from_slice(&self.buf[a..=b]);
    }

    /// §598: `dvi_out`.
    #[inline]
    fn out_byte(&mut self, c: u8) -> Result<(), TooLong> {
        self.buf[ux(self.ptr)] = c;
        self.ptr += 1;
        if self.ptr == self.limit {
            self.swap()?;
        }
        Ok(())
    }

    /// `dvi_out` of an `i32` that tex.web knows is a byte.
    fn out_int(&mut self, c: i32) -> Result<(), TooLong> {
        self.out_byte(u8::try_from(c & 0xFF).unwrap_or(0))
    }

    /// §598: output half of the buffer.
    fn swap(&mut self) -> Result<(), TooLong> {
        if self.ptr > 0x7FFF_FFFF - self.offset {
            self.cur_s = -2;
            return Err(TooLong);
        }
        if self.limit == self.buf_size {
            self.write(0, self.half_buf - 1);
            self.limit = self.half_buf;
            self.offset += self.buf_size;
            self.ptr = 0;
        } else {
            self.write(self.half_buf, self.buf_size - 1);
            self.limit = self.buf_size;
        }
        self.gone += self.half_buf;
        Ok(())
    }

    /// §600
    fn four(&mut self, x: i32) -> Result<(), TooLong> {
        for b in x.to_be_bytes() {
            self.out_byte(b)?;
        }
        Ok(())
    }

    /// §601
    fn pop(&mut self, l: i32) -> Result<(), TooLong> {
        if l == self.offset + self.ptr && self.ptr > 0 {
            self.ptr -= 1;
            Ok(())
        } else {
            self.out_byte(POP)
        }
    }

    /// §602
    fn font_def(&mut self, d: &FontDef) -> Result<(), TooLong> {
        let n = d.font - FONT_BASE - 1;
        if let Some(def) = &d.native {
            // `XeTeX`'s `dvi_native_font_def`
            self.out_byte(DEFINE_NATIVE_FONT)?;
            self.four(n)?;
            for &c in def {
                self.out_byte(c)?;
            }
            return Ok(());
        }
        if d.font <= 256 + FONT_BASE {
            self.out_byte(FNT_DEF1)?;
            self.out_int(n)?;
        } else {
            self.out_byte(FNT_DEF1 + 1)?;
            self.out_int(n / 0o400)?;
            self.out_int(n % 0o400)?;
        }
        for b in d.check {
            self.out_byte(b)?;
        }
        self.four(d.size)?;
        self.four(d.design_size)?;
        self.out_int(i32::try_from(d.area.len()).unwrap_or(0))?;
        self.out_int(i32::try_from(d.name.len()).unwrap_or(0))?;
        // §603: output the font name.
        for &c in d.area.iter().chain(&d.name) {
            self.out_byte(c)?;
        }
        Ok(())
    }

    /// §621: change font `dvi_f` to `f`, defining it at its first use.
    fn change_font(&mut self, f: i32) -> Result<(), TooLong> {
        let entry = self.fonts[ux(f)].as_mut().expect("page lists its fonts");
        if !entry.1 {
            entry.1 = true;
            let def = entry.0.clone();
            self.font_def(&def)?;
        }
        let n = f - FONT_BASE - 1;
        if f <= 64 + FONT_BASE {
            self.out_int(n + i32::from(FNT_NUM_0))?;
        } else if f <= 256 + FONT_BASE {
            self.out_byte(FNT1)?;
            self.out_int(n)?;
        } else {
            self.out_byte(FNT1 + 1)?;
            self.out_int(n / 0o400)?;
            self.out_int(n % 0o400)?;
        }
        self.dvi_f = f;
        Ok(())
    }

    /// §607: produce a DVI command for a movement of `w` (`o` is `down1`
    /// or `right1`), reusing `w`/`x`/`y`/`z` registers when possible.
    fn movement(&mut self, mut w: Scaled, o: u8) -> Result<(), TooLong> {
        let location = self.offset + self.ptr;
        let stack = if o == DOWN1 {
            &mut self.down
        } else {
            &mut self.right
        };
        stack.push(Movement {
            width: w,
            location,
            info: YZ_OK,
        });
        let q = stack.len() - 1; // the new entry, on top
        // §611: look at the other stack entries until deciding what sort of
        // DVI command to generate; `found` is a "hit".
        let mut mstate = NONE_SEEN;
        let mut found = None;
        let mut retag = None;
        for p in (0..q).rev() {
            let m = stack[p];
            if m.width == w {
                // §612: consider an entry with matching width.
                match mstate + m.info {
                    x if x == NONE_SEEN + YZ_OK
                        || x == NONE_SEEN + Y_OK
                        || x == Z_SEEN + YZ_OK
                        || x == Z_SEEN + Y_OK =>
                    {
                        if m.location < self.gone {
                            break;
                        }
                        // §613: change buffered instruction to `y` or `w`.
                        retag = Some((p, Y1 - DOWN1, Y_HERE));
                        found = Some(p);
                        break;
                    }
                    x if x == NONE_SEEN + Z_OK || x == Y_SEEN + YZ_OK || x == Y_SEEN + Z_OK => {
                        if m.location < self.gone {
                            break;
                        }
                        // §614: change buffered instruction to `z` or `x`.
                        retag = Some((p, Z1 - DOWN1, Z_HERE));
                        found = Some(p);
                        break;
                    }
                    x if x == NONE_SEEN + Y_HERE
                        || x == NONE_SEEN + Z_HERE
                        || x == Y_SEEN + Z_HERE
                        || x == Z_SEEN + Y_HERE =>
                    {
                        found = Some(p);
                        break;
                    }
                    _ => {}
                }
            } else {
                match mstate + m.info {
                    x if x == NONE_SEEN + Y_HERE => mstate = Y_SEEN,
                    x if x == NONE_SEEN + Z_HERE => mstate = Z_SEEN,
                    x if x == Y_SEEN + Z_HERE || x == Z_SEEN + Y_HERE => break,
                    _ => {}
                }
            }
        }
        if let Some((p, delta, tag)) = retag {
            let mut k = stack[p].location - self.offset;
            if k < 0 {
                k += self.buf_size;
            }
            let k = ux(k);
            self.buf[k] = self.buf[k].wrapping_add(delta);
            stack[p].info = tag;
        }
        if let Some(p) = found {
            // §609: generate a `y0` or `z0` command in order to reuse a
            // previous appearance of `w`.
            let info = stack[p].info;
            stack[q].info = info;
            let (op, fix) = if info == Y_HERE {
                (o + (Y0 - DOWN1), [(YZ_OK, Z_OK), (Y_OK, D_FIXED)]) // `y0` or `w0`
            } else {
                (o + (Z0 - DOWN1), [(YZ_OK, Y_OK), (Z_OK, D_FIXED)]) // `z0` or `x0`
            };
            for m in &mut stack[p + 1..q] {
                for (from, to) in fix {
                    if m.info == from {
                        m.info = to;
                        break;
                    }
                }
            }
            return self.out_byte(op);
        }
        // §610: generate a `down` or `right` command for `w` and return.
        if w.abs() >= 0o40000000 {
            self.out_byte(o + 3)?; // `down4` or `right4`
            return self.four(w);
        }
        if w.abs() >= 0o100000 {
            self.out_byte(o + 2)?; // `down3` or `right3`
            if w < 0 {
                w += 0o100000000;
            }
            self.out_int(w / 0o200000)?;
            w %= 0o200000;
            self.out_int(w / 0o400)?;
            return self.out_int(w % 0o400);
        }
        if w.abs() >= 0o200 {
            self.out_byte(o + 1)?; // `down2` or `right2`
            if w < 0 {
                w += 0o200000;
            }
            self.out_int(w / 0o400)?;
            return self.out_int(w % 0o400);
        }
        self.out_byte(o)?; // `down1` or `right1`
        if w < 0 {
            w += 0o400;
        }
        self.out_int(w % 0o400)
    }

    /// §615: delete movement entries with `location>=l`.
    fn prune_movements(&mut self, l: i32) {
        while self.down.last().is_some_and(|m| m.location >= l) {
            self.down.pop();
        }
        while self.right.last().is_some_and(|m| m.location >= l) {
            self.right.pop();
        }
    }

    /// §616: `synch_h`.
    fn synch_h(&mut self) -> Result<(), TooLong> {
        if self.cur_h != self.dvi_h {
            self.movement(self.cur_h - self.dvi_h, RIGHT1)?;
            self.dvi_h = self.cur_h;
        }
        Ok(())
    }

    /// §616: `synch_v`.
    fn synch_v(&mut self) -> Result<(), TooLong> {
        if self.cur_v != self.dvi_v {
            self.movement(self.cur_v - self.dvi_v, DOWN1)?;
            self.dvi_v = self.cur_v;
        }
        Ok(())
    }

    fn set_char(&mut self, c: i32) -> Result<(), TooLong> {
        if c >= 128 {
            self.out_byte(SET1)?;
        }
        self.out_int(c)
    }

    /// §619, §629: output the box whose header is `page.items[i]`; `false`
    /// if it holds an [`Item::Cut`], which leaves it (and the boxes around
    /// it) open.
    fn list_out(&mut self, page: &Page, i: usize) -> Result<bool, TooLong> {
        let b = head(page, i);
        self.cur_s += 1;
        if self.cur_s > 0 {
            self.out_byte(PUSH)?;
        }
        if self.cur_s > self.max_push {
            self.max_push = self.cur_s;
        }
        let save_loc = self.offset + self.ptr;
        let items = i + 1..i + 1 + b.len;
        let complete = if b.vertical {
            self.vlist_out(page, &b, items)?
        } else {
            self.hlist_out(page, &b, items)?
        };
        if !complete {
            return Ok(false);
        }
        self.prune_movements(save_loc);
        if self.cur_s > 0 {
            self.pop(save_loc)?;
        }
        self.cur_s -= 1;
        Ok(true)
    }

    /// §619
    fn hlist_out(&mut self, page: &Page, b: &Head, items: Range<usize>) -> Result<bool, TooLong> {
        let base_line = self.cur_v;
        // e-TeX: a display line in right-to-left text is output left to
        // right (its contents are in visual order already otherwise).
        let display_in_rtl = b.display && self.rtl;
        if display_in_rtl {
            self.rtl = false;
            self.cur_h -= b.width;
        }
        let mut left_edge = self.cur_h;
        let mut i = items.start;
        while i < items.end {
            // §620, §622: output `item` for `hlist_out`.
            match page.items[i] {
                Item::Char {
                    font,
                    ch,
                    width,
                    raise,
                } => {
                    self.synch_h()?;
                    if raise == 0 {
                        self.synch_v()?;
                    }
                    if font != self.dvi_f {
                        self.change_font(font)?;
                    }
                    if raise != 0 {
                        self.cur_v = base_line - raise;
                        self.synch_v()?;
                        self.set_char(ch)?;
                        self.cur_v = base_line;
                    } else {
                        self.set_char(ch)?;
                    }
                    self.cur_h += width;
                    self.dvi_h = self.cur_h;
                }
                Item::Missing { font } => {
                    self.synch_h()?;
                    self.synch_v()?;
                    if font != self.dvi_f {
                        self.change_font(font)?;
                    }
                }
                Item::Move(w) => self.cur_h += w,
                Item::Rule {
                    height,
                    depth,
                    width,
                } => self.hlist_rule(base_line, height, depth, width)?,
                Item::Box { .. } => {
                    // §623: output a box in an hlist.
                    let b = head(page, i);
                    let save_h = self.dvi_h;
                    let save_v = self.dvi_v;
                    self.cur_v = base_line + b.shift; // shift the box down
                    let edge = self.cur_h + b.width;
                    if self.rtl {
                        self.cur_h = edge;
                    }
                    if !self.list_out(page, i)? {
                        return Ok(false);
                    }
                    self.dvi_h = save_h;
                    self.dvi_v = save_v;
                    self.cur_h = edge;
                    self.cur_v = base_line;
                    i += b.len;
                }
                Item::Edge { width, dist, rtl } => {
                    self.cur_h += width;
                    left_edge = self.cur_h + dist;
                    self.rtl = rtl;
                }
                Item::Leaders { kind, size } => {
                    // §626: output leaders in an hlist.
                    i += 1;
                    let leader = head(page, i);
                    let leader_wd = leader.width;
                    let wd = size + 10; // compensate for floating-point rounding
                    if self.rtl {
                        self.cur_h -= 10;
                    }
                    let edge = self.cur_h + wd;
                    let mut lx = 0;
                    // §627: let `cur_h` be the position of the first box.
                    if kind == LeaderKind::Aligned {
                        let save_h = self.cur_h;
                        self.cur_h = left_edge + leader_wd * ((self.cur_h - left_edge) / leader_wd);
                        if self.cur_h < save_h {
                            self.cur_h += leader_wd;
                        }
                    } else {
                        let lq = wd / leader_wd; // the number of box copies
                        let lr = wd % leader_wd; // the remaining space
                        if kind == LeaderKind::Centered {
                            self.cur_h += lr / 2;
                        } else {
                            lx = lr / (lq + 1);
                            self.cur_h += (lr - (lq - 1) * lx) / 2;
                        }
                    }
                    while self.cur_h + leader_wd <= edge {
                        // §628: output a leader box at `cur_h`, then
                        // advance `cur_h` by `leader_wd+lx`.
                        self.cur_v = base_line + leader.shift;
                        self.synch_v()?;
                        let save_v = self.dvi_v;
                        self.synch_h()?;
                        let save_h = self.dvi_h;
                        if self.rtl {
                            self.cur_h += leader_wd;
                        }
                        self.list_out(page, i)?;
                        self.dvi_v = save_v;
                        self.dvi_h = save_h;
                        self.cur_v = base_line;
                        self.cur_h = save_h + leader_wd + lx;
                    }
                    self.cur_h = if self.rtl { edge } else { edge - 10 };
                    i += leader.len;
                }
                Item::Special { start, len } => self.special(page.special(start, len))?,
                Item::Native {
                    font,
                    width,
                    start,
                    len,
                    ..
                } => {
                    self.synch_h()?;
                    self.synch_v()?;
                    if font != self.dvi_f {
                        self.change_font(font)?;
                    }
                    self.native(page, start, len)?;
                    self.cur_h += width;
                    self.dvi_h = self.cur_h;
                }
                Item::Cut => return Ok(false),
            }
            i += 1;
        }
        if display_in_rtl {
            self.rtl = true;
        }
        Ok(true)
    }

    /// §624: output a rule in an hlist, then `move_past`.
    fn hlist_rule(
        &mut self,
        base_line: Scaled,
        rule_ht: Scaled,
        rule_dp: Scaled,
        rule_wd: Scaled,
    ) -> Result<(), TooLong> {
        let rule_ht = rule_ht + rule_dp; // this is the rule thickness
        if rule_ht > 0 && rule_wd > 0 {
            // we don't output empty rules
            self.synch_h()?;
            self.cur_v = base_line + rule_dp;
            self.synch_v()?;
            self.out_byte(SET_RULE)?;
            self.four(rule_ht)?;
            self.four(rule_wd)?;
            self.cur_v = base_line;
            self.dvi_h += rule_wd;
        }
        // move_past:
        self.cur_h += rule_wd;
        Ok(())
    }

    /// §629
    fn vlist_out(
        &mut self,
        page: &Page,
        this_box: &Head,
        items: Range<usize>,
    ) -> Result<bool, TooLong> {
        let left_edge = self.cur_h;
        self.cur_v -= this_box.height;
        let top_edge = self.cur_v;
        let mut i = items.start;
        while i < items.end {
            // §631: output `item` for `vlist_out`.
            match page.items[i] {
                // (the engine reports characters in vlists as a confusion;
                // edges are in hlists only)
                Item::Char { .. } | Item::Missing { .. } | Item::Edge { .. } => {}
                Item::Move(h) => self.cur_v += h,
                Item::Rule {
                    height,
                    depth,
                    width,
                } => {
                    self.vlist_rule(height, depth, width)?;
                    self.cur_h = left_edge;
                }
                Item::Box { .. } => {
                    // §632: output a box in a vlist.
                    let b = head(page, i);
                    self.cur_v += b.height;
                    self.synch_v()?;
                    let save_h = self.dvi_h;
                    let save_v = self.dvi_v;
                    // shift the box right (e-TeX: left in right-to-left text)
                    self.cur_h = if self.rtl {
                        left_edge - b.shift
                    } else {
                        left_edge + b.shift
                    };
                    if !self.list_out(page, i)? {
                        return Ok(false);
                    }
                    self.dvi_h = save_h;
                    self.dvi_v = save_v;
                    self.cur_v = save_v + b.depth;
                    self.cur_h = left_edge;
                    i += b.len;
                }
                Item::Leaders { kind, size } => {
                    // §635: output leaders in a vlist.
                    i += 1;
                    let leader = head(page, i);
                    let leader_ht = leader.height + leader.depth;
                    let ht = size + 10; // compensate for floating-point rounding
                    let edge = self.cur_v + ht;
                    let mut lx = 0;
                    // §636: let `cur_v` be the position of the first box.
                    if kind == LeaderKind::Aligned {
                        let save_v = self.cur_v;
                        self.cur_v = top_edge + leader_ht * ((self.cur_v - top_edge) / leader_ht);
                        if self.cur_v < save_v {
                            self.cur_v += leader_ht;
                        }
                    } else {
                        let lq = ht / leader_ht; // the number of box copies
                        let lr = ht % leader_ht; // the remaining space
                        if kind == LeaderKind::Centered {
                            self.cur_v += lr / 2;
                        } else {
                            lx = lr / (lq + 1);
                            self.cur_v += (lr - (lq - 1) * lx) / 2;
                        }
                    }
                    while self.cur_v + leader_ht <= edge {
                        // §637: output a leader box at `cur_v`, then
                        // advance `cur_v` by `leader_ht+lx`.
                        self.cur_h = if self.rtl {
                            left_edge - leader.shift
                        } else {
                            left_edge + leader.shift
                        };
                        self.synch_h()?;
                        let save_h = self.dvi_h;
                        self.cur_v += leader.height;
                        self.synch_v()?;
                        let save_v = self.dvi_v;
                        self.list_out(page, i)?;
                        self.dvi_v = save_v;
                        self.dvi_h = save_h;
                        self.cur_h = left_edge;
                        self.cur_v = save_v - leader.height + leader_ht + lx;
                    }
                    self.cur_v = edge - 10;
                    i += leader.len;
                }
                Item::Special { start, len } => self.special(page.special(start, len))?,
                Item::Native {
                    font,
                    height,
                    depth,
                    start,
                    len,
                    ..
                } => {
                    self.cur_v += height;
                    self.cur_h = left_edge;
                    self.synch_h()?;
                    self.synch_v()?;
                    if font != self.dvi_f {
                        self.change_font(font)?;
                    }
                    self.native(page, start, len)?;
                    self.cur_v += depth;
                    self.cur_h = left_edge;
                }
                Item::Cut => return Ok(false),
            }
            i += 1;
        }
        Ok(true)
    }

    /// `XeTeX`: a native glyph command's bytes.
    fn native(&mut self, page: &Page, start: u32, len: u32) -> Result<(), TooLong> {
        let (a, n) = (start as usize, len as usize);
        for i in a..a + n {
            self.out_byte(page.native[i])?;
        }
        Ok(())
    }

    /// §633: output a rule in a vlist.
    fn vlist_rule(
        &mut self,
        rule_ht: Scaled,
        rule_dp: Scaled,
        rule_wd: Scaled,
    ) -> Result<(), TooLong> {
        let rule_ht = rule_ht + rule_dp; // this is the rule thickness
        self.cur_v += rule_ht;
        if rule_ht > 0 && rule_wd > 0 {
            // we don't output empty rules
            if self.rtl {
                self.cur_h -= rule_wd;
            }
            self.synch_h()?;
            self.synch_v()?;
            self.out_byte(PUT_RULE)?;
            self.four(rule_ht)?;
            self.four(rule_wd)?;
        }
        Ok(())
    }

    /// §1368: output a `\special`.
    fn special(&mut self, bytes: &[u8]) -> Result<(), TooLong> {
        self.synch_h()?;
        self.synch_v()?;
        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        if len < 256 {
            self.out_byte(XXX1)?;
            self.out_int(len)?;
        } else {
            self.out_byte(XXX4)?;
            self.four(len)?;
        }
        for &c in bytes {
            self.out_byte(c)?;
        }
        Ok(())
    }
}

/// The dimensions of the box whose header is `page.items[i]`.
struct Head {
    display: bool,
    vertical: bool,
    width: Scaled,
    height: Scaled,
    depth: Scaled,
    shift: Scaled,
    len: usize,
}

#[inline]
fn head(page: &Page, i: usize) -> Head {
    let Item::Box {
        vertical,
        width,
        height,
        depth,
        shift,
        len,
        display,
    } = page.items[i]
    else {
        panic!("page item {i} is not a box");
    };
    Head {
        display,
        vertical,
        width,
        height,
        depth,
        shift,
        len: len as usize,
    }
}

#[inline]
fn ux(i: i32) -> usize {
    usize::try_from(i).expect("DVI buffer index")
}
