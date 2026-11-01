//! Text: the show operators as pdfwrite turns them into PDF text
//! (gdevpdtt.c `pdf_text_process`, gdevpdte.c `pdf_process_string`,
//! gdevpdts.c's text state: `Tf`, `Tm`/`Td`/`TL`, `Tc`, `Tw`, `Tr`,
//! `Tj`/`TJ`), and `stringwidth`/`charpath` as gs's own text code runs
//! them.

use alloc::vec;
use alloc::vec::Vec;

use crate::interp::{E, Frame, OpFn, Ps, R, Sig};
use crate::matrix::{IDENT, M};
use crate::num::pg;
use crate::obj::{Obj, PDict, V};
use crate::path::{f2x_round, x2f};
use crate::pdf::{Ctx, Device};

/// `pdf_text_state_values_t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Values {
    pub character_spacing: f32,
    /// The PDF font resource (an index into the device's fonts).
    pub font: Option<usize>,
    pub size: f32,
    pub matrix: M,
    pub render_mode: i32,
    pub word_spacing: f32,
}

impl Default for Values {
    fn default() -> Values {
        Values {
            character_spacing: 0.0,
            font: None,
            size: 0.0,
            matrix: IDENT,
            render_mode: 0,
            word_spacing: 0.0,
        }
    }
}

/// `pdf_text_state_t`.
#[derive(Clone, Debug)]
pub struct TextState {
    pub in_: Values,
    pub start: (f64, f64),
    pub chars: Vec<u8>,
    pub moves: Vec<(usize, f64)>,
    pub wmode: i32,
    pub out: Values,
    pub leading: f32,
    pub use_leading: bool,
    pub continue_line: bool,
    pub line_start: (f64, f64),
    pub out_pos: (f64, f64),
    pub can_use_tj: bool,
}

impl Default for TextState {
    fn default() -> TextState {
        TextState {
            in_: Values::default(),
            start: (0.0, 0.0),
            chars: Vec::new(),
            moves: Vec::new(),
            wmode: 0,
            out: Values::default(),
            leading: 0.0,
            use_leading: false,
            continue_line: false,
            line_start: (0.0, 0.0),
            out_pos: (0.0, 0.0),
            can_use_tj: true,
        }
    }
}

const MAX_TEXT_BUFFER_CHARS: usize = 200;
const MAX_TEXT_BUFFER_MOVES: usize = 50;

impl TextState {
    /// `pdf_from_stream_to_text`.
    pub fn from_stream_to_text(&mut self) {
        self.out.matrix = IDENT;
        self.line_start = (0.0, 0.0);
        self.continue_line = false;
        self.chars.clear();
        self.moves.clear();
    }
    /// `pdf_reset_text_state`.
    pub fn reset_text(&mut self) {
        self.out = Values::default();
        self.leading = 0.0;
    }
}

fn compatible(a: &M, b: &M) -> bool {
    a.xx == b.xx && a.xy == b.xy && a.yx == b.yx && a.yy == b.yy
}

/// `set_text_distance`.
fn text_distance(dx: f64, dy: f64, m: &M) -> (f64, f64) {
    let (mut x, mut y) = m.idtransform(dx, dy).unwrap_or((0.0, 0.0));
    let r = libm::floor(x + 0.5);
    if (x - r).abs() < 0.0005 {
        x = r;
    }
    let r = libm::floor(y + 0.5);
    if (y - r).abs() < 0.0005 {
        y = r;
    }
    (x, y)
}

/// `append_text_move`.
fn append_text_move(t: &mut TextState, dw: f64) -> bool {
    let pos = t.chars.len();
    let mut dw = dw;
    if let Some(&(i, a)) = t.moves.last()
        && i == pos
    {
        dw += a;
        t.moves.pop();
    }
    let r = libm::floor(dw + 0.5);
    if (dw - r).abs() < 0.001 {
        dw = r;
    }
    if dw != 0.0 {
        if t.moves.len() == MAX_TEXT_BUFFER_MOVES {
            return false;
        }
        t.moves.push((pos, dw));
    }
    true
}

/// `add_text_delta_move`.
fn add_text_delta_move(t: &mut TextState, m: &M) -> bool {
    if !compatible(m, &t.in_.matrix) {
        return false;
    }
    let dx = f64::from(m.tx) - f64::from(t.in_.matrix.tx);
    let dy = f64::from(m.ty) - f64::from(t.in_.matrix.ty);
    let (x, y) = text_distance(dx, dy, m);
    let (dw, dnotw) = if t.wmode != 0 { (y, x) } else { (x, y) };
    let tdw = dw * -1000.0 / f64::from(t.in_.size);
    if t.can_use_tj && dnotw == 0.0 && !t.chars.is_empty() && append_text_move(t, tdw) {
        t.in_.matrix = *m;
        return true;
    }
    false
}

/// `pdf_set_text_matrix`.
fn set_text_matrix(d: &mut Device) {
    let t = &mut d.text;
    t.use_leading = false;
    let mut s = alloc::string::String::new();
    if compatible(&t.out.matrix, &t.in_.matrix) {
        let (x, y) = text_distance(
            t.start.0 - t.line_start.0,
            t.start.1 - t.line_start.1,
            &t.in_.matrix,
        );
        if x == 0.0 && y < 0.0 {
            let dist_y = (-y) as f32;
            if (t.leading - dist_y).abs() > 0.0005 {
                s.push_str(&alloc::format!("{} TL\n", pg(f64::from(dist_y))));
                t.leading = dist_y;
            }
            t.use_leading = true;
        } else {
            s.push_str(&alloc::format!("{} {} Td\n", pg(x), pg(y)));
        }
    } else {
        let m = t.in_.matrix;
        let (sx, sy) = (0.1f64, 0.1f64);
        let (mut ax, mut bx, mut ay, mut by) = (sx, sx, sy, sy);
        if m.xx != 0.0 && f64::from(m.xx).abs() * ax < 0.00000001 {
            ax = libm::ceil(0.00000001 / f64::from(m.xx));
        }
        if m.xy != 0.0 && f64::from(m.xy).abs() * ay < 0.00000001 {
            ay = libm::ceil(0.00000001 / f64::from(m.xy));
        }
        if m.yx != 0.0 && f64::from(m.yx).abs() * bx < 0.00000001 {
            bx = libm::ceil(0.00000001 / f64::from(m.yx));
        }
        if m.yy != 0.0 && f64::from(m.yy).abs() * by < 0.00000001 {
            by = libm::ceil(0.00000001 / f64::from(m.yy));
        }
        s.push_str(&alloc::format!(
            "{} {} {} {} {} {} Tm\n",
            pg(f64::from(m.xx) * ax),
            pg(f64::from(m.xy) * ay),
            pg(f64::from(m.yx) * bx),
            pg(f64::from(m.yy) * by),
            pg(t.start.0 * sx),
            pg(t.start.1 * sy)
        ));
    }
    t.line_start = t.start;
    t.out.matrix = t.in_.matrix;
    d.put(&s);
}

/// A PDF string as `pdf_put_string` writes it (gs's `s_PSSE`).
pub fn pdf_string(b: &[u8]) -> Vec<u8> {
    let mut o = vec![b'('];
    for &c in b {
        match c {
            b'(' | b')' | b'\\' => {
                o.push(b'\\');
                o.push(c);
            }
            b'\n' => o.extend_from_slice(b"\\n"),
            b'\r' => o.extend_from_slice(b"\\r"),
            b'\t' => o.extend_from_slice(b"\\t"),
            0x08 => o.extend_from_slice(b"\\b"),
            0x0c => o.extend_from_slice(b"\\f"),
            32..=126 => o.push(c),
            _ => o.extend_from_slice(alloc::format!("\\{c:03o}").as_bytes()),
        }
    }
    o.push(b')');
    o
}

/// `flush_text_buffer`.
fn flush_text_buffer_inner(d: &mut Device) {
    if !d.text.chars.is_empty()
        && let Some(f) = d.text.in_.font
    {
        let id = crate::pdffont::assign_id(d, f);
        d.add_resource("Font", id);
    }
    let chars = core::mem::take(&mut d.text.chars);
    let moves = core::mem::take(&mut d.text.moves);
    if moves.is_empty() {
        let s = pdf_string(&chars);
        d.putb(&s);
        d.put(if d.text.use_leading { "'\n" } else { "Tj\n" });
    } else {
        if d.text.use_leading {
            d.put("T*");
        }
        d.put("[");
        let mut cur = 0;
        for &(next, amount) in &moves {
            let s = pdf_string(&chars[cur..next]);
            d.putb(&s);
            d.put(&pg(amount));
            cur = next;
        }
        if chars.len() > cur {
            let s = pdf_string(&chars[cur..]);
            d.putb(&s);
        }
        d.put("]TJ\n");
    }
    d.text.use_leading = false;
}

/// `sync_text_state` (also `pdf_from_string_to_text`).
pub fn flush_text_buffer(d: &mut Device) {
    if d.text.chars.is_empty() {
        return;
    }
    if d.text.continue_line {
        flush_text_buffer_inner(d);
        return;
    }
    let (i, o) = (d.text.in_, d.text.out);
    if o.character_spacing != i.character_spacing {
        d.put(&alloc::format!(
            "{} Tc\n",
            pg(f64::from(i.character_spacing))
        ));
        d.text.out.character_spacing = i.character_spacing;
    }
    if o.font != i.font || o.size != i.size {
        if let Some(f) = i.font {
            let id = crate::pdffont::assign_id(d, f);
            d.put(&alloc::format!("/R{id} {} Tf\n", pg(f64::from(i.size))));
        }
        d.text.out.font = i.font;
        d.text.out.size = i.size;
        d.text.wmode = 0;
    }
    let t = &d.text;
    if i.matrix != t.out.matrix
        || ((t.start != t.out_pos) && (!t.chars.is_empty() || !t.moves.is_empty()))
    {
        set_text_matrix(d);
    }
    if d.text.out.render_mode != i.render_mode {
        d.put(&alloc::format!("{} Tr\n", i.render_mode));
        d.text.out.render_mode = i.render_mode;
    }
    if d.text.out.word_spacing != i.word_spacing && d.text.chars.contains(&32) {
        d.put(&alloc::format!("{} Tw\n", pg(f64::from(i.word_spacing))));
        d.text.out.word_spacing = i.word_spacing;
    }
    flush_text_buffer_inner(d);
}

/// `pdf_close_text_contents`.
pub fn close_text(d: &mut Device) {
    d.text.in_.font = None;
    d.text.out.font = None;
    d.text.in_.size = 0.0;
    d.text.out.size = 0.0;
}

/// `pdf_set_text_state_values`.
fn set_text_state_values(d: &mut Device, v: &Values) {
    if !d.text.chars.is_empty() {
        let i = d.text.in_;
        if i.character_spacing == v.character_spacing
            && i.font == v.font
            && i.size == v.size
            && i.render_mode == v.render_mode
            && i.word_spacing == v.word_spacing
        {
            if i.matrix == v.matrix {
                return;
            }
            if add_text_delta_move(&mut d.text, &v.matrix) {
                return;
            }
        }
        flush_text_buffer(d);
    }
    d.text.in_ = *v;
    d.text.continue_line = false;
}

/// `pdf_append_chars`.
fn append_chars(d: &mut Device, s: &[u8], wx: f64, wy: f64) {
    if d.text.chars.is_empty() && d.text.moves.is_empty() {
        let p = (
            f64::from(d.text.in_.matrix.tx),
            f64::from(d.text.in_.matrix.ty),
        );
        d.text.out_pos = p;
        d.text.start = p;
    }
    let mut left = s;
    while !left.is_empty() {
        if d.text.chars.len() == MAX_TEXT_BUFFER_CHARS {
            flush_text_buffer(d);
            d.text.continue_line = true;
        } else {
            d.open_page(Ctx::String);
            let n = (MAX_TEXT_BUFFER_CHARS - d.text.chars.len()).min(left.len());
            d.text.chars.extend_from_slice(&left[..n]);
            left = &left[n..];
        }
    }
    d.text.in_.matrix.tx = (f64::from(d.text.in_.matrix.tx) + wx) as f32;
    d.text.in_.matrix.ty = (f64::from(d.text.in_.matrix.ty) + wy) as f32;
    d.text.out_pos.0 += wx;
    d.text.out_pos.1 += wy;
}

/// How a show operator moves after each character.
#[derive(Clone, Copy, Default)]
struct Adjust {
    all: Option<(f64, f64)>,
    space: Option<(f64, f64, u8)>,
}

impl Ps<'_> {
    /// The current font, Courier if none was set.
    fn cur_font(&mut self) -> R<alloc::rc::Rc<crate::font::Font>> {
        if let Some(f) = self.gs.font.clone() {
            return Ok(f);
        }
        let n = self.nobj("Courier");
        let fo = self.findfont(&n)?;
        let V::Dict(d) = &fo.v else {
            return Err(E::InvalidFont.into());
        };
        self.font_of(d).ok_or(Sig::Err(E::InvalidFont))
    }

    /// A glyph's name for a character code through the font's Encoding.
    pub fn glyph_name(&mut self, d: &PDict, c: u8) -> Vec<u8> {
        match self.dget_s(d, "Encoding").map(|o| o.v) {
            Some(V::Arr(a)) if usize::from(c) < a.len => match a.get(usize::from(c)).v {
                V::Name(n) => self.names.get(n).to_vec(),
                V::Str(s) => s.bytes(),
                _ => b".notdef".to_vec(),
            },
            _ => b".notdef".to_vec(),
        }
    }

    /// A glyph's width in character space (the charstring's, or
    /// Metrics'), the .notdef's for a missing glyph.
    pub fn glyph_width(&mut self, d: &PDict, name: &[u8]) -> (f64, f64) {
        match self.type1_glyph(d, name) {
            Some(g) => g.width,
            None => self
                .type1_glyph(d, b".notdef")
                .map_or((0.0, 0.0), |g| g.width),
        }
    }

    /// The text matrix values of `pdf_update_text_state`.
    fn text_values(&mut self, font: &crate::font::Font, pdf_font: usize) -> R<Values> {
        let orig = font.orig_matrix;
        let smat = orig.invert().unwrap_or(IDENT);
        let smat = smat.mul(&font.matrix);
        let mut ctm = self.gs.ctm;
        ctm.tx = 0.0;
        ctm.ty = 0.0;
        let mut tmat = smat.mul(&ctm);
        let mut size = (libm::hypot(f64::from(tmat.yx), f64::from(tmat.yy)) / 10.0) as f32;
        if size < 0.01 {
            size = (libm::hypot(f64::from(tmat.xx), f64::from(tmat.xy)) / 10.0) as f32;
        }
        if size < 0.01 {
            size = 1.0;
        }
        let (cx, cy) = self.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
        let (fx, fy) = (x2f(f2x_round(cx)), x2f(f2x_round(cy)));
        tmat.xx /= size;
        tmat.xy /= size;
        tmat.yx /= size;
        tmat.yy /= size;
        tmat.tx = (f64::from(tmat.tx) + fx) as f32;
        tmat.ty = (f64::from(tmat.ty) + fy) as f32;
        Ok(Values {
            character_spacing: 0.0,
            font: Some(pdf_font),
            size,
            matrix: tmat,
            render_mode: 0,
            word_spacing: 0.0,
        })
    }

    /// Shows string `s` through pdfwrite (`pdf_process_string`).
    fn show_pdf(&mut self, s: &[u8], adj: Adjust) -> R {
        self.second_page()?;
        let font = self.cur_font()?;
        if font.font_type == 3 || font.font_type == 0 {
            return self.show_type3(&font, s, adj);
        }
        let marking = self.marking();
        // (pdf_prepare_text_drawing)
        if marking {
            let gs = self.gs.clone();
            let new_clip = self.dev.must_put_clip(&gs.clip);
            if new_clip {
                self.dev.unclip();
            } else if self.dev.ctx == Ctx::None {
                self.dev.open_page(Ctx::Stream);
            }
            self.dev.prepare(&gs, false);
            if new_clip {
                self.dev.put_clip(&gs.clip);
            }
            self.dev.set_color(&gs.color, false);
        }
        let pf = crate::pdffont::obtain(self, &font, s)?;
        let mut v = self.text_values(&font, pf)?;
        let size = f64::from(v.size);
        let smat = font.orig_matrix.invert().unwrap_or(IDENT).mul(&font.matrix);
        let mut ctm_only = self.gs.ctm;
        ctm_only.tx = 0.0;
        ctm_only.ty = 0.0;
        let mut emulate = false;
        if let Some((ax, ay)) = adj.all {
            match smat.idtransform(ax, ay) {
                Some((x, y)) if y == 0.0 => v.character_spacing = (x * size) as f32,
                _ => emulate = true,
            }
        }
        if let Some((sx, sy, ch)) = adj.space {
            match smat.idtransform(sx, sy) {
                Some((x, y)) if y == 0.0 && ch == 32 => v.word_spacing = (x * size) as f32,
                _ => emulate = s.contains(&ch),
            }
        }
        if emulate {
            return Err(
                self.unsupported("ashow/widthshow spacing gs emulates character by character")
            );
        }
        if emulate {
            // (process_text_modify_width: a character at a time)
            for &c in s {
                let mut a = adj;
                if !s.is_empty() {
                    a.all = adj.all;
                }
                self.show_pdf_one(&font, pf, c, a)?;
            }
            return Ok(());
        }
        if marking {
            let dev = &mut self.dev;
            dev.text.can_use_tj = true;
            set_text_state_values(dev, &v);
        }
        // (process_text_return_width)
        let mut w = (0.0f64, 0.0f64);
        let mut spaces = 0;
        for &c in s {
            let (rw, _) = crate::pdffont::char_width(self, pf, &font, c);
            w.0 += rw / 1000.0;
            if adj.space.is_some_and(|(_, _, ch)| ch == c) {
                spaces += 1;
            }
        }
        let (mut dx, mut dy) = v.matrix.dtransform(w.0 * size, w.1 * size);
        if let Some((ax, ay)) = adj.all {
            let (tx, ty) = ctm_only.dtransform(ax, ay);
            dx += tx * s.len() as f64;
            dy += ty * s.len() as f64;
        }
        if let Some((sx, sy, _)) = adj.space {
            let (tx, ty) = ctm_only.dtransform(sx, sy);
            dx += tx * f64::from(spaces);
            dy += ty * f64::from(spaces);
        }
        if marking {
            append_chars(&mut self.dev, s, dx, dy);
            crate::pdffont::mark_used(&mut self.dev, pf, s);
        }
        // (pdf_shift_text_currentpoint)
        let (cx, cy) = self.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
        let (ox, oy) = (x2f(f2x_round(cx)), x2f(f2x_round(cy)));
        self.moveto_dev(ox + dx, oy + dy)
    }

    /// One character with its own advance.
    fn show_pdf_one(&mut self, font: &crate::font::Font, pf: usize, c: u8, adj: Adjust) -> R {
        let mut v = self.text_values(font, pf)?;
        v.character_spacing = 0.0;
        let size = f64::from(v.size);
        if self.marking() {
            self.dev.text.can_use_tj = true;
            set_text_state_values(&mut self.dev, &v);
        }
        let (rw, _) = crate::pdffont::char_width(self, pf, font, c);
        let (mut dx, mut dy) = v.matrix.dtransform(rw / 1000.0 * size, 0.0);
        let mut ctm_only = self.gs.ctm;
        ctm_only.tx = 0.0;
        ctm_only.ty = 0.0;
        if let Some((ax, ay)) = adj.all {
            let (tx, ty) = ctm_only.dtransform(ax, ay);
            dx += tx;
            dy += ty;
        }
        if let Some((sx, sy, ch)) = adj.space
            && ch == c
        {
            let (tx, ty) = ctm_only.dtransform(sx, sy);
            dx += tx;
            dy += ty;
        }
        if self.marking() {
            let (wx, wy) = v.matrix.dtransform(rw / 1000.0 * size, 0.0);
            append_chars(&mut self.dev, &[c], wx, wy);
            crate::pdffont::mark_used(&mut self.dev, pf, &[c]);
        }
        let (cx, cy) = self.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
        let (ox, oy) = (x2f(f2x_round(cx)), x2f(f2x_round(cy)));
        self.moveto_dev(ox + dx, oy + dy)
    }

    /// A Type 3 (or composite) font's characters: their procedures run
    /// as graphics (pdfwrite makes a Type 3 font of them; not ported).
    fn show_type3(&mut self, font: &crate::font::Font, s: &[u8], adj: Adjust) -> R {
        if self.marking() {
            return Err(self.unsupported(if font.font_type == 0 {
                "composite (Type 0) fonts"
            } else {
                "Type 3 fonts"
            }));
        }
        let d = font.dict.clone();
        for &c in s {
            let (cx, cy) = self.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
            let saved = self.gs.clone();
            let fm = font.matrix.mul(&self.gs.ctm);
            self.gs.ctm = M {
                tx: cx as f32,
                ty: cy as f32,
                ..fm
            };
            self.newpath();
            self.gs.char_width = None;
            let bg = self.dget_s(&d, "BuildGlyph");
            let r = if let Some(bg) = bg {
                let name = self.glyph_name(&d, c);
                self.push(Obj::dict(d.clone()));
                let n = Obj::name(self.names.intern(&name));
                self.push(n);
                self.call(bg)
            } else if let Some(bc) = self.dget_s(&d, "BuildChar") {
                self.push(Obj::dict(d.clone()));
                self.pushi(i64::from(c));
                self.call(bc)
            } else {
                Err(E::InvalidFont.into())
            };
            let w = self.gs.char_width.unwrap_or((0.0, 0.0));
            self.gs = saved;
            r?;
            let (mut dx, mut dy) = fm.dtransform(w.0, w.1);
            let mut ctm_only = self.gs.ctm;
            ctm_only.tx = 0.0;
            ctm_only.ty = 0.0;
            if let Some((ax, ay)) = adj.all {
                let (tx, ty) = ctm_only.dtransform(ax, ay);
                dx += tx;
                dy += ty;
            }
            if let Some((sx, sy, ch)) = adj.space
                && ch == c
            {
                let (tx, ty) = ctm_only.dtransform(sx, sy);
                dx += tx;
                dy += ty;
            }
            self.moveto_dev(cx + dx, cy + dy)?;
        }
        Ok(())
    }

    /// The device advance of a string as gs's text code sums it (cached
    /// widths in fixed point).
    fn string_advance(&mut self, s: &[u8]) -> R<(f64, f64)> {
        let font = self.cur_font()?;
        let d = font.dict.clone();
        let fm = font.matrix.mul(&self.gs.ctm);
        let (mut sx, mut sy) = (0i64, 0i64);
        for &c in s {
            let w = if font.font_type == 3 {
                (0.0, 0.0)
            } else {
                let name = self.glyph_name(&d, c);
                self.glyph_width(&d, &name)
            };
            let (dx, dy) = fm.dtransform(w.0, w.1);
            sx += i64::from(crate::path::f2x(dx));
            sy += i64::from(crate::path::f2x(dy));
        }
        Ok((sx as f64 / 256.0, sy as f64 / 256.0))
    }

    /// Appends the glyph outlines of `s` to the path (`charpath`).
    fn charpath(&mut self, s: &[u8]) -> R {
        let font = self.cur_font()?;
        let d = font.dict.clone();
        for &c in s {
            let (cx, cy) = self.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
            let fm = font.matrix.mul(&self.gs.ctm);
            let m = M {
                tx: cx as f32,
                ty: cy as f32,
                ..fm
            };
            let name = self.glyph_name(&d, c);
            let g = self
                .type1_glyph(&d, &name)
                .or_else(|| self.type1_glyph(&d, b".notdef"))
                .unwrap_or_default();
            for (op, p) in &g.path {
                match op {
                    0 => {
                        let (x, y) = m.transform(p[0], p[1]);
                        self.moveto_dev(x, y)?;
                    }
                    1 => {
                        let (x, y) = m.transform(p[0], p[1]);
                        self.lineto_dev(x, y)?;
                    }
                    2 => {
                        let a = m.transform(p[0], p[1]);
                        let b = m.transform(p[2], p[3]);
                        let e = m.transform(p[4], p[5]);
                        self.curveto_dev([a.0, a.1, b.0, b.1, e.0, e.1])?;
                    }
                    _ => self.closepath(),
                }
            }
            let (dx, dy) = fm.dtransform(g.width.0, g.width.1);
            self.moveto_dev(cx + dx, cy + dy)?;
        }
        Ok(())
    }
}

fn str_arg(ps: &Ps<'_>, i: usize) -> R<Vec<u8>> {
    match &ps.top(i)?.v {
        V::Str(s) => Ok(s.bytes()),
        _ => Err(E::TypeCheck.into()),
    }
}

fn op_show(ps: &mut Ps<'_>) -> R {
    let s = str_arg(ps, 0)?;
    ps.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
    ps.o.pop();
    ps.show_pdf(&s, Adjust::default())
}
fn op_ashow(ps: &mut Ps<'_>) -> R {
    let s = str_arg(ps, 0)?;
    let ay = ps.num_at(1)?;
    let ax = ps.num_at(2)?;
    ps.o.truncate(ps.o.len() - 3);
    ps.show_pdf(
        &s,
        Adjust {
            all: Some((ax, ay)),
            space: None,
        },
    )
}
fn op_widthshow(ps: &mut Ps<'_>) -> R {
    let s = str_arg(ps, 0)?;
    let c = ps.int_at(1)?;
    let cy = ps.num_at(2)?;
    let cx = ps.num_at(3)?;
    ps.o.truncate(ps.o.len() - 4);
    ps.show_pdf(
        &s,
        Adjust {
            all: None,
            space: Some((cx, cy, c as u8)),
        },
    )
}
fn op_awidthshow(ps: &mut Ps<'_>) -> R {
    let s = str_arg(ps, 0)?;
    let ay = ps.num_at(1)?;
    let ax = ps.num_at(2)?;
    let c = ps.int_at(3)?;
    let cy = ps.num_at(4)?;
    let cx = ps.num_at(5)?;
    ps.o.truncate(ps.o.len() - 6);
    ps.show_pdf(
        &s,
        Adjust {
            all: Some((ax, ay)),
            space: Some((cx, cy, c as u8)),
        },
    )
}
/// `xshow`, `yshow`, `xyshow`: each character, then a move by the
/// numbers given.
fn xyshow(ps: &mut Ps<'_>, mode: u8) -> R {
    if ps.marking() {
        return Err(ps.unsupported("xshow, yshow and xyshow"));
    }
    let nums = ps.top(0)?.clone();
    let s = str_arg(ps, 1)?;
    let v: Vec<f64> = match &nums.v {
        V::Arr(_) => ps.nums_of(&nums)?,
        V::Str(st) => crate::upath::num_string(&st.bytes()).ok_or(Sig::Err(E::TypeCheck))?,
        _ => return Err(E::TypeCheck.into()),
    };
    ps.o.truncate(ps.o.len() - 2);
    let mut k = 0;
    for &c in &s {
        let (cx, cy) = ps.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
        ps.show_pdf(&[c], Adjust::default())?;
        let (dx, dy) = match mode {
            0 => (v.get(k).copied().unwrap_or(0.0), 0.0),
            1 => (0.0, v.get(k).copied().unwrap_or(0.0)),
            _ => (
                v.get(k).copied().unwrap_or(0.0),
                v.get(k + 1).copied().unwrap_or(0.0),
            ),
        };
        k += if mode == 2 { 2 } else { 1 };
        let (ddx, ddy) = ps.gs.ctm.dtransform(dx, dy);
        ps.moveto_dev(cx + ddx, cy + ddy)?;
    }
    Ok(())
}
fn op_xshow(ps: &mut Ps<'_>) -> R {
    xyshow(ps, 0)
}
fn op_yshow(ps: &mut Ps<'_>) -> R {
    xyshow(ps, 1)
}
fn op_xyshow(ps: &mut Ps<'_>) -> R {
    xyshow(ps, 2)
}
fn op_kshow(ps: &mut Ps<'_>) -> R {
    if ps.marking() {
        return Err(ps.unsupported("kshow"));
    }
    let s = str_arg(ps, 0)?;
    let p = ps.top(1)?.clone();
    ps.o.truncate(ps.o.len() - 2);
    for (i, &c) in s.iter().enumerate() {
        ps.show_pdf(&[c], Adjust::default())?;
        if i + 1 < s.len() {
            ps.pushi(i64::from(c));
            ps.pushi(i64::from(s[i + 1]));
            ps.call(p.clone())?;
        }
    }
    Ok(())
}
fn op_cshow(ps: &mut Ps<'_>) -> R {
    let s = str_arg(ps, 0)?;
    let p = ps.top(1)?.clone();
    ps.o.truncate(ps.o.len() - 2);
    for &c in &s {
        let font = ps.cur_font()?;
        let d = font.dict.clone();
        let name = ps.glyph_name(&d, c);
        let w = ps.glyph_width(&d, &name);
        let (wx, wy) = font.matrix.dtransform(w.0, w.1);
        ps.pushi(i64::from(c));
        ps.pushr(wx);
        ps.pushr(wy);
        ps.call(p.clone())?;
    }
    Ok(())
}
fn op_glyphshow(ps: &mut Ps<'_>) -> R {
    let g = ps.pop()?;
    let name = match &g.v {
        V::Name(n) => ps.names.get(*n).to_vec(),
        _ => return Err(E::TypeCheck.into()),
    };
    let font = ps.cur_font()?;
    let d = font.dict.clone();
    let code = (0..=255u8).find(|&c| ps.glyph_name(&d, c) == name);
    match code {
        Some(c) => ps.show_pdf(&[c], Adjust::default()),
        None if ps.marking() => {
            Err(ps.unsupported("glyphshow of a glyph the font's Encoding lacks"))
        }
        None => Ok(()),
    }
}
fn op_stringwidth(ps: &mut Ps<'_>) -> R {
    let s = str_arg(ps, 0)?;
    let (dx, dy) = ps.string_advance(&s)?;
    let (x, y) = ps
        .gs
        .ctm
        .idtransform(dx, dy)
        .ok_or(Sig::Err(E::UndefinedResult))?;
    ps.o.pop();
    ps.pushr(x);
    ps.pushr(y);
    Ok(())
}
fn op_charpath(ps: &mut Ps<'_>) -> R {
    if ps.marking() {
        return Err(ps.unsupported("charpath (glyph outlines as paths)"));
    }
    let _stroke = ps.pop_bool()?;
    let s = str_arg(ps, 0)?;
    ps.o.pop();
    ps.charpath(&s)
}
fn op_settextrendering(ps: &mut Ps<'_>) -> R {
    ps.pop_int()?;
    Ok(())
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, OpFn)] = &[
        ("show", op_show),
        ("ashow", op_ashow),
        ("widthshow", op_widthshow),
        ("awidthshow", op_awidthshow),
        ("xshow", op_xshow),
        ("yshow", op_yshow),
        ("xyshow", op_xyshow),
        ("kshow", op_kshow),
        ("cshow", op_cshow),
        ("glyphshow", op_glyphshow),
        ("stringwidth", op_stringwidth),
        ("charpath", op_charpath),
        (".settextrenderingmode", op_settextrendering),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
    let _ = Frame::Barrier;
}
