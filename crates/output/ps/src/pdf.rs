//! Ghostscript's pdfwrite (devices/vector/gdevpdf*.c) for one page: the
//! content stream as it writes it (contexts, the viewer's graphics state
//! and its `q`/`Q`, colors, line parameters, clipping, paths), the
//! resources named `R<object number>`, and the file: objects numbered in
//! pdfwrite's order, non-stream objects in object streams, a
//! cross-reference stream, the XMP metadata.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::color::{Color, Space};
use crate::gstate::{Clip, GState};
use crate::matrix::{IDENT, M};
use crate::num::pg;
use crate::path::{P, Path, Pe, Rect, x2f};

/// What the command line set (gs's device parameters).
#[derive(Clone, Debug)]
pub struct Settings {
    /// `-dCompatibilityLevel` (1.5: 15).
    pub level: i32,
    /// `-dPDFSETTINGS=/prepress`.
    pub prepress: bool,
    /// `-dEPSCrop`.
    pub eps_crop: bool,
    /// The paper's size in points (`-sPAPERSIZE`), before a setpagedevice.
    pub paper: (f64, f64),
    /// Images: `-dAutoFilter*Images=false` with `/FlateEncode`.
    pub flate_images: bool,
    /// `SOURCE_DATE_EPOCH` (gs's clock with the reproducibility patch).
    pub epoch: i64,
    /// The output file's name (`-sOutputFile`), as gs records it.
    pub out_name: Vec<u8>,
    /// The `%%Invocation:` comment's words.
    pub invocation: Vec<Vec<u8>>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            level: 15,
            prepress: false,
            eps_crop: false,
            paper: (612.0, 792.0),
            flate_images: false,
            epoch: 0,
            out_name: Vec::new(),
            invocation: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Ctx {
    None,
    Stream,
    Text,
    String,
}

/// The viewer's state as pdfwrite tracks it (`pdf_viewer_state`).
#[derive(Clone)]
pub struct Viewer {
    pub fill: Saved,
    pub stroke: Saved,
    pub half_width: f32,
    pub cap: i32,
    pub join: i32,
    pub miter: f32,
    pub dash: Vec<f32>,
    pub dash_offset: f32,
    pub stroke_adjust: i32,
    pub flatness: f32,
    pub smoothness: f32,
    pub fill_overprint: bool,
    pub stroke_overprint: bool,
    pub overprint_mode: i32,
    pub clip_id: u64,
    pub clip_path: Option<Path>,
    pub halftone_id: u32,
    pub transfer_id: u64,
}

/// A saved color (`gx_hl_saved_color`): the initial black is a device
/// color, never equal to a client's.
#[derive(Clone)]
pub enum Saved {
    Black,
    Client(Color),
}

impl Saved {
    fn same(&self, c: &Color) -> bool {
        match self {
            Saved::Black => false,
            Saved::Client(s) => s.same(c),
        }
    }
}

/// A resource of the page (`/R<n>`).
#[derive(Clone)]
pub struct Res {
    pub kind: &'static str,
    pub id: i64,
}

/// The document being written.
pub struct Device {
    pub s: Settings,
    /// The page in device pixels (720 dpi).
    pub width: i32,
    pub height: i32,
    /// The page's size in points.
    pub media: (f64, f64),
    /// gs's object ids (clip paths, ...): identity only.
    pub ids: u64,
    /// The next PDF object number.
    pub next_obj: i64,
    pub ctx: Ctx,
    /// The page's content stream, uncompressed.
    pub cs: Vec<u8>,
    pub contents_id: i64,
    pub contents_length_id: i64,
    pub page_id: i64,
    pub v: Viewer,
    pub vg_initial: Viewer,
    pub vgstack: Vec<Viewer>,
    pub no_clip_id: u64,
    /// The page's resources, by category, in order of first use.
    pub page_res: Vec<Res>,
    pub procset_text: bool,
    pub procset_image_b: bool,
    pub procset_image_c: bool,
    pub procset_image_i: bool,
    /// Objects written to the object stream so far: (number, bytes).
    pub objstm: Vec<(i64, Vec<u8>)>,
    pub objstm_id: i64,
    /// Objects written directly (streams), in order: (number, bytes from
    /// `N 0 obj` to `endobj\n`).
    pub direct: Vec<(i64, Vec<u8>)>,
    /// ObjStm objects flushed (their stream object written in `direct`).
    pub objstm_done: Vec<(i64, Vec<(i64, usize)>)>,
    /// ExtGState objects by content (`pdf_substitute_resource`).
    pub extgstates: Vec<(Vec<u8>, i64)>,
    pub marked: bool,
    pub pages_done: usize,
    pub page_written: Vec<u8>,
    pub dash_obj: Option<crate::obj::Obj>,
    pub cs_obj: Option<crate::obj::Obj>,
    pub text: crate::text::TextState,
    pub fonts: crate::pdffont::Fonts,
    pub images: crate::image::Images,
    /// The default matrix's origin offset (Margins): none.
    pub page_open: bool,
    /// The first `showpage` ran: later marks go to pages not written.
    pub page_done: bool,
    /// What the page used that is not written as gs writes it.
    pub unsupported: Option<String>,
    /// The DSC comments gs puts in the Info dictionary (`%%Title`,
    /// `%%Creator`, `%%For` as `/Author`), in the order first seen.
    pub dsc_info: Vec<(&'static str, Vec<u8>)>,
}

pub fn initial_viewer() -> Viewer {
    Viewer {
        fill: Saved::Black,
        stroke: Saved::Black,
        half_width: 0.5,
        cap: 0,
        join: 0,
        miter: 10.0,
        dash: Vec::new(),
        dash_offset: 0.0,
        stroke_adjust: -1,
        flatness: 1.0,
        smoothness: 0.02,
        fill_overprint: false,
        stroke_overprint: false,
        overprint_mode: 0,
        clip_id: 0,
        clip_path: None,
        halftone_id: 1,
        transfer_id: 0,
    }
}

impl Device {
    pub fn new(s: Settings) -> Device {
        let mut d = Device {
            s,
            width: 0,
            height: 0,
            media: (0.0, 0.0),
            ids: 1000,
            next_obj: 4,
            ctx: Ctx::None,
            cs: Vec::new(),
            contents_id: 0,
            contents_length_id: 0,
            page_id: 0,
            v: initial_viewer(),
            vg_initial: initial_viewer(),
            vgstack: Vec::new(),
            no_clip_id: 0,
            page_res: Vec::new(),
            procset_text: false,
            procset_image_b: false,
            procset_image_c: false,
            procset_image_i: false,
            objstm: Vec::new(),
            objstm_id: 0,
            direct: Vec::new(),
            objstm_done: Vec::new(),
            extgstates: Vec::new(),
            marked: false,
            pages_done: 0,
            page_written: Vec::new(),
            dash_obj: None,
            cs_obj: None,
            text: crate::text::TextState::default(),
            fonts: crate::pdffont::Fonts::default(),
            images: crate::image::Images::default(),
            page_open: false,
            page_done: false,
            unsupported: None,
            dsc_info: Vec::new(),
        };
        let p = d.s.paper;
        d.set_page_size(p.0, p.1);
        d
    }

    /// The page's size (setpagedevice's `/PageSize`).
    pub fn set_page_size(&mut self, w: f64, h: f64) {
        self.media = (w, h);
        self.width = (w * 10.0 + 0.5) as i32;
        self.height = (h * 10.0 + 0.5) as i32;
    }

    /// The device's default matrix: 720 dpi, y up.
    pub fn default_matrix(&self) -> M {
        M {
            xx: 10.0,
            xy: 0.0,
            yx: 0.0,
            yy: 10.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    pub fn alloc(&mut self) -> i64 {
        let n = self.next_obj;
        self.next_obj += 1;
        n
    }

    pub fn put(&mut self, s: &str) {
        self.cs.extend_from_slice(s.as_bytes());
    }
    pub fn putb(&mut self, s: &[u8]) {
        self.cs.extend_from_slice(s);
    }

    // ------ objects ------

    /// Writes a non-stream object (into the object stream).
    pub fn write_separate(&mut self, id: i64, body: Vec<u8>) {
        if self.objstm.len() >= 200 {
            self.flush_objstm();
        }
        if self.objstm_id == 0 {
            self.objstm_id = self.alloc();
        }
        self.objstm.push((id, body));
    }

    /// `FlushObjStm`.
    pub fn flush_objstm(&mut self) {
        if self.objstm_id == 0 {
            return;
        }
        let mut offsets = String::new();
        let mut data = Vec::new();
        for (id, b) in &self.objstm {
            offsets.push_str(&format!("{id} {} ", data.len()));
            data.extend_from_slice(b);
        }
        let mut all = offsets.clone().into_bytes();
        all.extend_from_slice(&data);
        let z = phitex_codecs::deflate::compress(&all, 6);
        let mut o = format!(
            "{} 0 obj\n<</Filter/FlateDecode\n/Type/ObjStm\n/N {}\n/First {}/Length {}>>stream\n",
            self.objstm_id,
            self.objstm.len(),
            offsets.len(),
            z.len()
        )
        .into_bytes();
        o.extend_from_slice(&z);
        o.extend_from_slice(b"\nendstream\nendobj\n");
        let id = self.objstm_id;
        let members: Vec<(i64, usize)> = self
            .objstm
            .iter()
            .enumerate()
            .map(|(k, (i, _))| (*i, k))
            .collect();
        self.direct.push((id, o));
        self.objstm_done.push((id, members));
        self.objstm.clear();
        self.objstm_id = 0;
    }

    /// Writes a stream object directly: `dict` the dictionary's elements
    /// (without `<<`), the data as given.
    pub fn write_stream(&mut self, id: i64, dict: &str, data: &[u8]) {
        let mut o = format!("{id} 0 obj\n<<{dict}/Length {}>>stream\n", data.len()).into_bytes();
        o.extend_from_slice(data);
        o.extend_from_slice(b"\nendstream\nendobj\n");
        self.direct.push((id, o));
    }

    // ------ contexts ------

    /// `pdf_open_page`.
    pub fn open_page(&mut self, ctx: Ctx) {
        if !self.page_open {
            self.page_open = true;
            self.page_id = self.alloc();
        }
        self.open_contents(ctx);
    }

    /// `pdf_open_contents`.
    pub fn open_contents(&mut self, ctx: Ctx) {
        while self.ctx != ctx {
            match (self.ctx, ctx) {
                (Ctx::None, _) => self.none_to_stream(),
                (Ctx::Stream, Ctx::None) => self.stream_to_none(),
                (Ctx::Stream, _) => self.stream_to_text(),
                (Ctx::Text, Ctx::String) => self.ctx = Ctx::String,
                (Ctx::Text, _) => self.text_to_stream(),
                (Ctx::String, _) => self.string_to_text(),
            }
        }
    }

    fn none_to_stream(&mut self) {
        self.contents_id = self.alloc();
        self.contents_length_id = self.alloc();
        self.put("q 0.1 0 0 0.1 0 0 cm\n");
        self.ctx = Ctx::Stream;
    }
    fn stream_to_text(&mut self) {
        self.save_viewer_state();
        self.put("10 0 0 10 0 0 cm BT\n");
        self.procset_text = true;
        self.text.from_stream_to_text();
        self.ctx = Ctx::Text;
    }
    fn string_to_text(&mut self) {
        crate::text::flush_text_buffer(self);
        self.ctx = Ctx::Text;
    }
    fn text_to_stream(&mut self) {
        self.put("ET\n");
        self.restore_viewer_state();
        self.text.reset_text();
        self.ctx = Ctx::Stream;
    }
    fn stream_to_none(&mut self) {
        if !self.vgstack.is_empty() {
            self.restore_viewer_state();
        }
        self.ctx = Ctx::None;
    }

    /// `pdf_close_contents(pdev, true)`.
    pub fn close_contents(&mut self) {
        if self.ctx == Ctx::None {
            return;
        }
        self.open_contents(Ctx::Stream);
        self.put("Q\n");
        self.open_contents(Ctx::None);
    }

    // ------ the viewer state ------

    pub fn save_viewer_state(&mut self) {
        self.vgstack.push(self.v.clone());
        self.put("q\n");
    }
    pub fn restore_viewer_state(&mut self) {
        if let Some(v) = self.vgstack.pop() {
            self.put("Q\n");
            self.v = v;
        }
    }

    // ------ graphics state updates ------

    /// `pdf_prepare_drawing` and the overprint / stroke adjust parts of
    /// `pdf_try_prepare_fill` / `_stroke`: an ExtGState if anything
    /// changed. `stroke`: for stroking.
    pub fn prepare(&mut self, gs: &GState, stroke: bool) {
        let mut d: Vec<(String, String)> = Vec::new();
        if self.v.smoothness != gs.smoothness {
            d.push(("/SM".into(), pg(f64::from(gs.smoothness))));
            self.v.smoothness = gs.smoothness;
        }
        if stroke && self.v.stroke_adjust != i32::from(gs.stroke_adjust) {
            d.push((
                "/SA".into(),
                if gs.stroke_adjust {
                    "true".into()
                } else {
                    "false".into()
                },
            ));
            self.v.stroke_adjust = i32::from(gs.stroke_adjust);
        }
        self.end_gstate(d);
    }

    /// `pdf_end_gstate`: the ExtGState as a resource (an identical one
    /// reused), and `/R<n> gs`.
    fn end_gstate(&mut self, d: Vec<(String, String)>) {
        if d.is_empty() {
            return;
        }
        let mut body = String::from("<</Type/ExtGState");
        for (k, v) in &d {
            body.push('\n');
            body.push_str(k);
            if !v.starts_with(['/', '(', '<', '[']) {
                body.push(' ');
            }
            body.push_str(v);
        }
        body.push_str(">>");
        let id = self.alloc();
        let found = self
            .extgstates
            .iter()
            .find(|(b, _)| *b == body.as_bytes())
            .map(|x| x.1);
        let id = match found {
            Some(old) => {
                // (the new one cancelled: its number stays used)
                old
            }
            None => {
                self.extgstates.push((body.clone().into_bytes(), id));
                self.write_separate(id, body.into_bytes());
                id
            }
        };
        self.open_page(Ctx::Stream);
        self.add_resource("ExtGState", id);
        self.put(&format!("/R{id} gs\n"));
    }

    pub fn add_resource(&mut self, kind: &'static str, id: i64) {
        if !self.page_res.iter().any(|r| r.kind == kind && r.id == id) {
            self.page_res.push(Res { kind, id });
        }
    }

    /// `pdf_set_drawing_color` for filling (`stroke` false) or stroking.
    pub fn set_color(&mut self, c: &Color, stroke: bool) {
        let saved = if stroke { &self.v.stroke } else { &self.v.fill };
        if saved.same(c) {
            return;
        }
        self.open_page(Ctx::Stream);
        let vals: Vec<String> = c
            .vals
            .iter()
            .map(|&v| pg(psdf_round(f64::from(v))))
            .collect();
        let s = vals.join(" ");
        match &c.cs.space {
            Space::Gray => self.put(&format!("{s} {}\n", if stroke { "G" } else { "g" })),
            Space::Rgb => self.put(&format!("{s} {}\n", if stroke { "RG" } else { "rg" })),
            Space::Cmyk => self.put(&format!("{s} {}\n", if stroke { "K" } else { "k" })),
            _ => {
                if self.unsupported.is_none() {
                    self.unsupported = Some("color spaces other than DeviceGray, DeviceRGB and DeviceCMYK".into());
                }
                crate::pdfcolor::write_color(self, c, stroke);
            }
        }
        if stroke {
            self.v.stroke = Saved::Client(c.clone());
        } else {
            self.v.fill = Saved::Client(c.clone());
        }
    }

    // ------ clipping ------

    /// `pdf_must_put_clip_path`.
    pub fn must_put_clip(&mut self, clip: &Clip) -> bool {
        if self.v.clip_id == clip.id {
            return false;
        }
        if self.includes_page(clip) && self.v.clip_id == self.no_clip_id {
            return false;
        }
        if self.is_same_clip(clip) {
            self.v.clip_id = clip.id;
            return false;
        }
        true
    }

    fn includes_page(&self, clip: &Clip) -> bool {
        let (a, b) = clip.inner;
        a.x <= 0 && a.y <= 0 && (self.width << 8) <= b.x && (self.height << 8) <= b.y
    }

    fn is_same_clip(&self, clip: &Clip) -> bool {
        match &self.v.clip_path {
            None => false,
            Some(p) => p.elements() == clip.as_path().elements(),
        }
    }

    /// `pdf_unclip`.
    pub fn unclip(&mut self) {
        self.open_page(Ctx::Stream);
        if !self.vgstack.is_empty() {
            self.restore_viewer_state();
            self.v.clip_path = None;
            self.v.clip_id = self.no_clip_id;
        }
    }

    /// `pdf_put_clip_path`.
    pub fn put_clip(&mut self, clip: &Clip) {
        if self.v.clip_id == clip.id {
            return;
        }
        let mut new_id = clip.id;
        if self.includes_page(clip) {
            if self.v.clip_id == self.no_clip_id {
                return;
            }
            new_id = self.no_clip_id;
        }
        if self.is_same_clip(clip) {
            self.v.clip_id = new_id;
            return;
        }
        self.open_contents(Ctx::Stream);
        if !self.vgstack.is_empty() {
            self.restore_viewer_state();
        }
        if new_id != self.no_clip_id {
            self.save_viewer_state();
            let rect = if clip.path_valid {
                clip.path.is_rectangular().map(|(a, b, _)| (a, b))
            } else {
                None
            };
            if let Some((a, b)) = rect {
                let w = x2f(b.x) - x2f(a.x);
                let h = x2f(b.y) - x2f(a.y);
                self.put(&format!(
                    "{} {} {} {} re",
                    pg(x2f(a.x)),
                    pg(x2f(a.y)),
                    pg(w),
                    pg(h)
                ));
                self.put(&format!(" {} n\n", if clip.rule <= 0 { "W" } else { "W*" }));
            } else if clip.path_valid || clip.list.is_none() {
                let p = clip.as_path();
                write_path(self, &p, PathType::Clip, None);
                self.put(&format!("{} n\n", if clip.rule <= 0 { "W" } else { "W*" }));
            } else {
                let mut e = clip.list.clone();
                while let Some(l) = e {
                    let segs = write_path(self, &l.path, PathType::Clip, None);
                    if segs > 0 {
                        self.put(&format!("{} n\n", if l.rule <= 0 { "W" } else { "W*" }));
                    }
                    e.clone_from(&l.next);
                }
            }
        }
        self.v.clip_id = new_id;
        self.v.clip_path = if new_id == self.no_clip_id {
            None
        } else {
            Some(clip.as_path())
        };
    }

    /// `prepare_fill_with_clip`: whether there is anything to paint.
    pub fn prepare_fill_with_clip(&mut self, gs: &GState, have_path: bool) -> bool {
        let (ca, cb) = gs.clip.outer;
        if ca.x >= cb.x || ca.y >= cb.y {
            return false;
        }
        let new_clip = self.must_put_clip(&gs.clip);
        if have_path || self.ctx == Ctx::None || new_clip {
            if new_clip {
                self.unclip();
            } else {
                self.open_page(Ctx::Stream);
            }
        }
        self.prepare(gs, false);
        self.put_clip(&gs.clip);
        true
    }

    // ------ painting ------

    /// `gdev_pdf_fill_path`.
    pub fn fill_path(&mut self, gs: &GState, path: &Path, eo: bool) {
        self.marked = true;
        let have_path = !path.is_void();
        if !self.prepare_fill_with_clip(gs, have_path) || !have_path {
            return;
        }
        self.set_color(&gs.color, false);
        if gs.flatness != self.v.flatness {
            self.put(&format!("{} i\n", pg(f64::from(gs.flatness))));
            self.v.flatness = gs.flatness;
        }
        write_path(self, path, PathType::Fill, None);
        self.put(if eo { "f*\n" } else { "f\n" });
    }

    /// `gdev_pdf_fill_rectangle_hl_color`.
    pub fn fill_rect_hl(&mut self, gs: &GState, a: P, b: P) {
        self.marked = true;
        if a.x == b.x {
            return;
        }
        if !self.prepare_fill_with_clip(gs, true) {
            return;
        }
        self.set_color(&gs.color, false);
        let (ca, cb) = gs.clip.outer;
        let (mut a, mut b) = (a, b);
        a.x = a.x.max(ca.x);
        a.y = a.y.max(ca.y);
        b.x = b.x.min(cb.x);
        b.y = b.y.min(cb.y);
        if a.x > b.x || a.y > b.y {
            return;
        }
        self.put(&format!(
            "{} {} {} {} re f\n",
            pg(x2f(a.x)),
            pg(x2f(a.y)),
            pg(x2f(b.x) - x2f(a.x)),
            pg(x2f(b.y) - x2f(a.y))
        ));
    }

    /// `gdev_pdf_stroke_path`.
    pub fn stroke_path(&mut self, gs: &GState, path: &Path) {
        if path.is_void() {
            return;
        }
        self.marked = true;
        if self.must_put_clip(&gs.clip) {
            self.unclip();
        } else {
            self.open_page(Ctx::Stream);
        }
        self.prepare(gs, true);
        self.put_clip(&gs.clip);
        let (set_ctm, scale, mat) = stroke_scaling(&gs.ctm);
        // (painting outside the clip: nothing)
        let (ba, bb) = path.bbox();
        let (d0x, d0y) = gs.ctm.dtransform(f64::from(gs.line.half_width), 0.0);
        let (d1x, d1y) = gs.ctm.dtransform(0.0, f64::from(gs.line.half_width));
        let ex = crate::path::f2x(d0x.abs()).max(crate::path::f2x(d1x.abs())) + 512;
        let ey = crate::path::f2x(d0y.abs()).max(crate::path::f2x(d1y.abs())) + 512;
        let (ca, cb) = gs.clip.outer;
        let (sa, sb) = (
            P {
                x: (ba.x - ex).max(ca.x),
                y: (ba.y - ey).max(ca.y),
            },
            P {
                x: (bb.x + ex).min(cb.x),
                y: (bb.y + ey).min(cb.y),
            },
        );
        if sb.x < sa.x || sb.y < sa.y {
            return;
        }
        self.prepare_stroke_params(gs, scale);
        self.set_color(&gs.color, true);
        if set_ctm {
            put_matrix(self, "q ", &mat, "cm\n");
        }
        let dashed = gs.line.dash_offset != 0.0 || !gs.line.dash.is_empty();
        write_path(
            self,
            path,
            if dashed {
                PathType::DashedStroke
            } else {
                PathType::Stroke
            },
            if set_ctm { Some(&mat) } else { None },
        );
        self.put(if set_ctm { "S Q\n" } else { "S\n" });
    }

    /// `gdev_vector_prepare_stroke` (without the color).
    pub fn prepare_stroke_params(&mut self, gs: &GState, scale: f64) {
        let dash_offset = (f64::from(gs.line.dash_offset) * scale) as f32;
        let half = (f64::from(gs.line.half_width) * scale) as f32;
        let pat: Vec<f32> = gs
            .line
            .dash
            .iter()
            .map(|&x| (f64::from(x) * scale) as f32)
            .collect();
        if dash_offset != self.v.dash_offset || pat != self.v.dash {
            let mut s = String::from("[ ");
            for &x in &pat {
                s.push_str(&pg(f64::from(x)));
                s.push(' ');
            }
            s.push_str(&format!("] {} d\n", pg(f64::from(dash_offset))));
            self.put(&s);
            self.v.dash = pat;
            self.v.dash_offset = dash_offset;
        }
        if half != self.v.half_width {
            self.put(&format!("{} w\n", pg((f64::from(half) * 2.0).abs())));
            self.v.half_width = half;
        }
        if gs.line.miter_limit != self.v.miter {
            self.put(&format!("{} M\n", pg(f64::from(gs.line.miter_limit))));
            self.v.miter = gs.line.miter_limit;
        }
        if gs.line.cap != self.v.cap {
            self.put(&format!("{} J\n", gs.line.cap));
            self.v.cap = gs.line.cap;
        }
        if gs.line.join != self.v.join {
            self.put(&format!("{} j\n", gs.line.join));
            self.v.join = gs.line.join;
        }
    }

    // ------ the page and the file ------

    /// `pdf_close_page` and the document's close: the whole file.
    pub fn finish_page(&mut self) {
        // (the page)
        if !self.page_open {
            self.page_id = self.alloc();
            self.page_open = true;
        }
        self.close_contents();
        crate::text::close_text(self);
        if self.contents_id == 0 {
            self.contents_id = self.alloc();
            self.contents_length_id = self.alloc();
            self.put("q 0.1 0 0 0.1 0 0 cm\nQ\n");
        }
        let content = core::mem::take(&mut self.cs);
        let z = phitex_codecs::deflate::compress(&content, 6);
        let mut o = format!(
            "{} 0 obj\n<</Length {} 0 R/Filter /FlateDecode>>\nstream\n",
            self.contents_id, self.contents_length_id
        )
        .into_bytes();
        o.extend_from_slice(&z);
        o.extend_from_slice(b"endstream\nendobj\n");
        self.direct.push((self.contents_id, o));
        self.write_separate(
            self.contents_length_id,
            format!("{}\n", z.len()).into_bytes(),
        );
        // (the page's resource dictionaries, by category)
        let mut cats: Vec<(&'static str, i64)> = Vec::new();
        for kind in [
            "ColorSpace",
            "ExtGState",
            "Pattern",
            "Shading",
            "XObject",
            "Font",
        ] {
            let rs: Vec<i64> = self
                .page_res
                .iter()
                .filter(|r| r.kind == kind)
                .map(|r| r.id)
                .collect();
            if rs.is_empty() {
                continue;
            }
            let id = self.alloc();
            let mut b = String::from("<<");
            for (k, r) in rs.iter().enumerate() {
                if k > 0 {
                    b.push('\n');
                }
                b.push_str(&format!("/R{r}\n{r} 0 R"));
            }
            b.push_str(">>\n");
            self.write_separate(id, b.into_bytes());
            cats.push((kind, id));
        }
        let mut page = format!(
            "{} 0 obj\n<</Type/Page/MediaBox [0 0 {} {}]\n/Parent 3 0 R\n/Resources<</ProcSet[/PDF",
            self.page_id,
            pg(round_box(self.media.0)),
            pg(round_box(self.media.1))
        );
        if self.procset_image_b {
            page.push_str(" /ImageB");
        }
        if self.procset_image_c {
            page.push_str(" /ImageC");
        }
        if self.procset_image_i {
            page.push_str(" /ImageI");
        }
        if self.procset_text {
            page.push_str(" /Text");
        }
        page.push_str("]\n");
        for (k, id) in &cats {
            page.push_str(&format!("/{k} {id} 0 R\n"));
        }
        page.push_str(&format!(
            ">>\n/Contents {} 0 R\n>>\nendobj\n",
            self.contents_id
        ));
        self.page_written = page.into_bytes();
    }

    /// The rest of the file: Pages, Catalog, Info, Metadata, the object
    /// stream and the cross-reference stream.
    pub fn document(&mut self) -> Vec<u8> {
        let date = pdf_date(self.s.epoch);
        let mut info = format!(
            "<</Producer(GPL Ghostscript 10.08.0)\n/CreationDate({date})\n/ModDate({date})"
        )
        .into_bytes();
        for (k, v) in &self.dsc_info {
            info.extend_from_slice(format!("\n/{k}").as_bytes());
            info.extend_from_slice(&crate::text::pdf_string(v));
        }
        info.extend_from_slice(b">>");
        let meta_id = self.alloc();
        self.write_separate(2, info);
        let get = |k: &str| self.dsc_info.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        let xmp = xmp(self.s.epoch, &self.file_id(), get("Title"), get("Creator"));
        let mut m = format!(
            "{meta_id} 0 obj\n<</Type/Metadata\n/Subtype/XML/Length {}>>stream\n",
            xmp.len()
        )
        .into_bytes();
        m.extend_from_slice(&xmp);
        m.extend_from_slice(b"\nendstream\nendobj\n");
        self.flush_objstm_end(m)
    }

    fn flush_objstm_end(&mut self, meta: Vec<u8>) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"%PDF-1.5\n%\xc7\xec\x8f\xa2\n");
        out.extend_from_slice(&invocation(&self.s.invocation));
        let mut offsets: BTreeMap<i64, (u8, i64, i64)> = BTreeMap::new();
        // (the contents stream first, then the page, Pages, Catalog)
        let direct = core::mem::take(&mut self.direct);
        let (first, rest): (Vec<_>, Vec<_>) = direct
            .into_iter()
            .partition(|(id, _)| *id == self.contents_id);
        for (id, b) in first {
            offsets.insert(id, (1, out.len() as i64, 0));
            out.extend_from_slice(&b);
        }
        offsets.insert(self.page_id, (1, out.len() as i64, 0));
        out.extend_from_slice(&self.page_written);
        offsets.insert(3, (1, out.len() as i64, 0));
        out.extend_from_slice(
            format!(
                "3 0 obj\n<< /Type /Pages /Kids [\n{} 0 R\n] /Count 1\n>>\nendobj\n",
                self.page_id
            )
            .as_bytes(),
        );
        let meta_id = self.next_obj - 1;
        offsets.insert(1, (1, out.len() as i64, 0));
        out.extend_from_slice(
            format!(
                "1 0 obj\n<</Type /Catalog /Pages 3 0 R\n/Metadata {meta_id} 0 R\n>>\nendobj\n"
            )
            .as_bytes(),
        );
        // (earlier flushed object streams were written where they were
        // flushed: before the rest)
        for (id, b) in rest {
            offsets.insert(id, (1, out.len() as i64, 0));
            out.extend_from_slice(&b);
        }
        offsets.insert(meta_id, (1, out.len() as i64, 0));
        out.extend_from_slice(&meta);
        self.flush_objstm();
        for (id, b) in core::mem::take(&mut self.direct) {
            offsets.insert(id, (1, out.len() as i64, 0));
            out.extend_from_slice(&b);
        }
        for (stm, members) in &self.objstm_done {
            for (id, k) in members {
                offsets.insert(*id, (2, *stm, *k as i64));
            }
        }
        // (the cross-reference stream)
        let xref_id = self.alloc();
        let size = xref_id + 1;
        let start = out.len() as i64;
        offsets.insert(xref_id, (1, start, 0));
        let w2 = if start + 1000 > 0xffff { 3 } else { 2 };
        let mut x = Vec::new();
        x.push(0u8);
        x.extend_from_slice(&be(0, w2));
        x.extend_from_slice(&be(0xffff, 2));
        for id in 1..size {
            let (t, a, b) = offsets.get(&id).copied().unwrap_or((0, 0, 0));
            x.push(t);
            x.extend_from_slice(&be(a, w2));
            x.extend_from_slice(&be(b, 2));
        }
        let z = phitex_codecs::deflate::compress(&x, 6);
        let fid = self.file_id();
        let h: String = fid.iter().map(|b| format!("{b:02X}")).collect();
        out.extend_from_slice(
            format!(
                "{xref_id} 0 obj\n<<\n/Type /XRef\n/Size {size}\n/Root 1 0 R /Info 2 0 R\n/ID [<{h}><{h}>]\n/Index [0 {size} ]\n/W [1 {w2} 2]\n/Filter /FlateDecode/Length {}\n>>\nstream\n",
                z.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&z);
        out.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{start}\n%%EOF\n").as_bytes(),
        );
        out
    }

    /// The file identifier: MD5 of gs's clock, the output's name and the
    /// Info dictionary (`pdf_compute_fileID`).
    pub fn file_id(&self) -> [u8; 16] {
        let mut d = Vec::new();
        d.extend_from_slice(&self.s.epoch.to_le_bytes());
        d.extend_from_slice(&0i64.to_le_bytes());
        d.extend_from_slice(&self.s.out_name);
        let date = pdf_date(self.s.epoch);
        d.extend_from_slice(
            format!("/Producer(GPL Ghostscript 10.08.0)/CreationDate({date})/ModDate({date})")
                .as_bytes(),
        );
        phitex_codecs::md5::md5(&d)
    }
}

fn be(v: i64, n: usize) -> Vec<u8> {
    let b = v.to_be_bytes();
    b[8 - n..].to_vec()
}

/// `round_box_coord`.
pub fn round_box(v: f64) -> f64 {
    f64::from((v * 100.0 + 0.5) as i32) / 100.0
}

/// `psdf_round(v, 255, 8)`.
pub fn psdf_round(v: f64) -> f64 {
    let mut mul = 1.0;
    let mut w = v;
    if w <= 0.0 {
        return w;
    }
    while w < 255.0 {
        w *= 8.0;
        mul *= 8.0;
    }
    f64::from((w + 0.5) as i32) / mul
}

/// The `%%Invocation:` comment, its lines broken as gs breaks them.
fn invocation(words: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%%Invocation:".to_vec();
    let mut line = out.len();
    for w in words {
        if line + 1 + w.len() > 255 {
            out.extend_from_slice(b"\n%%+");
            line = 3;
        }
        out.push(b' ');
        out.extend_from_slice(w);
        line += 1 + w.len();
    }
    out.push(b'\n');
    out
}

/// `D:yyyymmddhhmmssZ00'00'` of a time.
pub fn pdf_date(t: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(t);
    format!("D:{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}Z00'00'")
}

/// UTC calendar time of a Unix time.
pub fn civil(t: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 { y + 1 } else { y },
        m,
        d,
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
    )
}

/// The XMP metadata packet.
/// XML text of a DSC value (gs's `pdf_xmp_write_translated`: `&`, `<`,
/// `>` escaped, the bytes as UTF-8 where they are).
fn xml_text(v: &[u8]) -> String {
    let s = String::from_utf8_lossy(v);
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn xmp(t: i64, id: &[u8; 16], title: Option<Vec<u8>>, creator: Option<Vec<u8>>) -> Vec<u8> {
    let (y, mo, d, h, mi, s) = civil(t);
    let date = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z");
    let uuid = crate::xmp::uuid(id, t);
    let mut x = String::new();
    x.push_str("<?xpacket begin='\u{feff}' id='W5M0MpCehiHzreSzNTczkc9d'?>\n");
    x.push_str("<?adobe-xap-filters esc=\"CRLF\"?>\n");
    x.push_str(
        "<x:xmpmeta xmlns:x='adobe:ns:meta/' x:xmptk='XMP toolkit 2.9.1-13, framework 1.6'>\n",
    );
    x.push_str("<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#' xmlns:iX='http://ns.adobe.com/iX/1.0/'>\n");
    x.push_str("<rdf:Description rdf:about=\"\" xmlns:pdf='http://ns.adobe.com/pdf/1.3/' pdf:Producer='GPL Ghostscript 10.08.0'/>\n");
    x.push_str(&format!("<rdf:Description rdf:about=\"\" xmlns:xmp='http://ns.adobe.com/xap/1.0/'><xmp:ModifyDate>{date}</xmp:ModifyDate>\n"));
    x.push_str(&format!("<xmp:CreateDate>{date}</xmp:CreateDate>\n"));
    x.push_str(&format!("<xmp:MetadataDate>{date}</xmp:MetadataDate>\n"));
    match &creator {
        Some(c) => x.push_str(&format!("<xmp:CreatorTool>{}</xmp:CreatorTool></rdf:Description>\n", xml_text(c))),
        None => x.push_str("<xmp:CreatorTool>'UnknownApplication'</xmp:CreatorTool></rdf:Description>\n"),
    }
    x.push_str(&format!("<rdf:Description rdf:about=\"\" xmlns:xmpMM='http://ns.adobe.com/xap/1.0/mm/' xmpMM:DocumentID='{uuid}'/>\n"));
    x.push_str("<rdf:Description rdf:about=\"\" xmlns:xmpMM='http://ns.adobe.com/xap/1.0/mm/' xmpMM:RenditionClass='default'/>\n");
    x.push_str("<rdf:Description rdf:about=\"\" xmlns:xmpMM='http://ns.adobe.com/xap/1.0/mm/' xmpMM:VersionID='1'/>\n");
    let tl = title.as_ref().map_or_else(|| String::from("'Untitled'"), |t| xml_text(t));
    x.push_str(&format!("<rdf:Description rdf:about=\"\" xmlns:dc='http://purl.org/dc/elements/1.1/' dc:format='application/pdf'><dc:title><rdf:Alt><rdf:li xml:lang='x-default'>{tl}</rdf:li></rdf:Alt></dc:title></rdf:Description>\n"));
    x.push_str("</rdf:RDF>\n</x:xmpmeta>\n");
    let pad = " ".repeat(72);
    x.push_str(&pad);
    x.push('\n');
    x.push_str(&pad);
    x.push('\n');
    x.push_str("<?xpacket end='w'?>");
    x.into_bytes()
}

/// The kind of path being written (`gx_path_type_*`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PathType {
    Fill,
    Stroke,
    DashedStroke,
    Clip,
}

/// `gdev_vector_stroke_scaling`: whether a matrix must be set, the
/// scale, the matrix.
pub fn stroke_scaling(ctm: &M) -> (bool, f64, M) {
    let mut set_ctm = true;
    let mut scale = 1.0;
    let (xx, xy, yx, yy) = (
        f64::from(ctm.xx),
        f64::from(ctm.xy),
        f64::from(ctm.yx),
        f64::from(ctm.yy),
    );
    if ctm.is_xxyy() {
        scale = xx.abs();
        set_ctm = yy.abs() != scale;
    } else if ctm.is_xyyx() {
        scale = xy.abs();
        set_ctm = yx.abs() != scale;
    } else if (ctm.xx == ctm.yy && ctm.xy == -ctm.yx) || (ctm.xx == -ctm.yy && ctm.xy == ctm.yx) {
        scale = libm::hypot(xx, xy);
        set_ctm = false;
    }
    let mut mat = IDENT;
    if set_ctm {
        scale = 0.5 * (xx.abs() + xy.abs() + yx.abs() + yy.abs());
        mat = M {
            xx: (xx / scale) as f32,
            xy: (xy / scale) as f32,
            yx: (yx / scale) as f32,
            yy: (yy / scale) as f32,
            tx: 0.0,
            ty: 0.0,
        };
        if (ctm.xx == 0.0 && ctm.xy == 0.0) || (ctm.yx == 0.0 && ctm.yy == 0.0) {
            set_ctm = false;
            scale = (xx + xy + yx + yy).abs() / libm::sqrt(2.0);
        }
    }
    (set_ctm, scale, mat)
}

pub fn put_matrix(d: &mut Device, before: &str, m: &M, after: &str) {
    d.put(before);
    for v in m.to_vec() {
        d.put(&pg(f64::from(v)));
        d.put(" ");
    }
    d.put(after);
}

/// Writes a segment (`gdev_vector_dopath_segment` with `psdf_*`).
struct Seg<'a> {
    d: &'a mut Device,
    mat: M,
    first: bool,
    start: (f64, f64),
    prev: (f64, f64),
}

impl Seg<'_> {
    fn pt(&self, p: P) -> (f64, f64) {
        self.mat
            .itransform(x2f(p.x), x2f(p.y))
            .unwrap_or((0.0, 0.0))
    }
    fn seg(&mut self, e: Pe) {
        match e {
            Pe::Move(p) => {
                let v = self.pt(p);
                if self.first {
                    self.start = v;
                    self.first = false;
                }
                self.d.put(&format!("{} {} m\n", pg(v.0), pg(v.1)));
                self.prev = v;
            }
            Pe::Line(p) => {
                let v = self.pt(p);
                self.d.put(&format!("{} {} l\n", pg(v.0), pg(v.1)));
                self.prev = v;
            }
            Pe::Curve(a, b, c) => {
                let (v1, v2, v3) = (self.pt(a), self.pt(b), self.pt(c));
                let v0 = self.prev;
                if v1 == v0 && v2 == v3 {
                    self.d.put(&format!("{} {} l\n", pg(v3.0), pg(v3.1)));
                } else if v1 == v0 {
                    self.d.put(&format!(
                        "{} {} {} {} v\n",
                        pg(v2.0),
                        pg(v2.1),
                        pg(v3.0),
                        pg(v3.1)
                    ));
                } else if v3 == v2 {
                    self.d.put(&format!(
                        "{} {} {} {} y\n",
                        pg(v1.0),
                        pg(v1.1),
                        pg(v2.0),
                        pg(v2.1)
                    ));
                } else {
                    self.d.put(&format!(
                        "{} {} {} {} {} {} c\n",
                        pg(v1.0),
                        pg(v1.1),
                        pg(v2.0),
                        pg(v2.1),
                        pg(v3.0),
                        pg(v3.1)
                    ));
                }
                self.prev = v3;
            }
            Pe::Close(_) => {
                self.d.put("h\n");
                self.prev = self.start;
            }
        }
    }
}

fn op_of(e: &Pe) -> u8 {
    match e {
        Pe::Move(_) => 1,
        Pe::Line(_) => 2,
        Pe::Curve(..) => 3,
        Pe::Close(_) => 4,
    }
}

fn v0(e: &Pe) -> P {
    match *e {
        Pe::Move(p) | Pe::Line(p) | Pe::Close(p) => p,
        Pe::Curve(a, _, _) => a,
    }
}

fn set_v0(e: &mut Pe, p: P) {
    match e {
        Pe::Move(x) | Pe::Line(x) | Pe::Close(x) => *x = p,
        Pe::Curve(a, _, _) => *a = p,
    }
}

/// `pdf_write_path` with the optimisations (`gx_path_type_optimize`):
/// rectangles as `re`, collinear lines merged. The number of segments.
pub fn write_path(d: &mut Device, path: &Path, ty: PathType, pmat: Option<&M>) -> i32 {
    let scale_mat = match pmat {
        Some(m) => m.scale(1.0, 1.0),
        None => IDENT,
    };
    let els = path.elements();
    let mut segs = 0;
    let matrix_optimisable = pmat.is_none_or(|m| m.is_xxyy() || m.is_xyyx())
        && scale_mat.xx == 1.0
        && scale_mat.yy == 1.0
        && scale_mat.is_xxyy()
        && scale_mat.tx == 0.0
        && scale_mat.ty == 0.0;
    let mut buffering = matrix_optimisable;
    let do_close = matches!(
        ty,
        PathType::Clip | PathType::Stroke | PathType::DashedStroke
    );
    let stroke = matches!(ty, PathType::Stroke | PathType::DashedStroke);
    let rtype = path.is_rectangular();
    if let Some((a, b, t)) = rtype
        && (!stroke || t == Rect::Closed)
        && matrix_optimisable
    {
        let (px, py) = scale_mat
            .itransform(f64::from(a.x), f64::from(a.y))
            .unwrap_or((0.0, 0.0));
        let (qx, qy) = scale_mat
            .itransform(f64::from(b.x), f64::from(b.y))
            .unwrap_or((0.0, 0.0));
        let (x0, y0, x1, y1) = (px as i32, py as i32, qx as i32, qy as i32);
        d.put(&format!(
            "{} {} {} {} re\n",
            pg(x2f(x0)),
            pg(x2f(y0)),
            pg(x2f(x1) - x2f(x0)),
            pg(x2f(y1) - x2f(y0))
        ));
        return 1;
    }
    let mut w = Seg {
        d,
        mat: scale_mat,
        first: true,
        start: (0.0, 0.0),
        prev: (0.0, 0.0),
    };
    let mut segments: [Pe; 5] = [Pe::Move(P::default()); 5];
    let mut si: i32 = 0;
    let mut initial_m = false;
    let mut line_start = P::default();
    let mut stored_moveto = false;
    let mut idx = 0;
    let next = |idx: &mut usize| -> Option<Pe> {
        let e = els.get(*idx).copied();
        *idx += 1;
        e
    };
    loop {
        let pe = next(&mut idx);
        let Some(pe) = pe else {
            // (pe_op 0: the end)
            for s in segments.iter().take(si.max(0) as usize) {
                w.seg(*s);
            }
            if stored_moveto && stroke {
                w.seg(Pe::Move(line_start));
            }
            break;
        };
        if segs == 0 {
            segs = 1;
        }
        segments[si as usize] = pe;
        match pe {
            Pe::Move(p) => {
                if !buffering {
                    stored_moveto = true;
                    line_start = p;
                    si = -1;
                } else {
                    for s in segments.iter().take(si as usize) {
                        w.seg(*s);
                    }
                    line_start = p;
                    segments[0] = segments[si as usize];
                    si = 0;
                    initial_m = true;
                }
            }
            Pe::Line(_) => {
                if !buffering {
                    if stored_moveto {
                        w.seg(Pe::Move(line_start));
                        stored_moveto = false;
                    }
                    for s in segments.iter().take(si as usize + 1) {
                        w.seg(*s);
                    }
                    si = -1;
                } else {
                    if !initial_m {
                        for s in segments.iter().take(si as usize + 1) {
                            w.seg(*s);
                        }
                        buffering = false;
                        si = -1;
                    }
                    if si > 0 {
                        let i = si as usize;
                        if op_of(&segments[i - 1]) == 2 {
                            let (c, p) = (v0(&segments[i]), v0(&segments[i - 1]));
                            if c.x == p.x && c.x == line_start.x {
                                if p.y > line_start.y && c.y >= p.y {
                                    set_v0(&mut segments[i - 1], P { x: p.x, y: c.y });
                                    si -= 1;
                                } else if p.y < line_start.y && c.y <= p.y {
                                    set_v0(&mut segments[i - 1], P { x: p.x, y: c.y });
                                    si -= 1;
                                } else {
                                    line_start = p;
                                }
                            } else if c.y == p.y && c.y == line_start.y {
                                if p.x > line_start.x && c.x > p.x {
                                    set_v0(&mut segments[i - 1], P { x: c.x, y: p.y });
                                    si -= 1;
                                } else if p.x < line_start.x && c.x < p.x {
                                    set_v0(&mut segments[i - 1], P { x: c.x, y: p.y });
                                    si -= 1;
                                } else {
                                    line_start = p;
                                }
                            } else {
                                line_start = p;
                            }
                        }
                    }
                }
            }
            Pe::Curve(_, _, c) => {
                if stored_moveto {
                    w.seg(Pe::Move(line_start));
                    stored_moveto = false;
                }
                for s in segments.iter().take(si as usize + 1) {
                    w.seg(*s);
                }
                line_start = c;
                si = -1;
                buffering = false;
            }
            Pe::Close(_) => {
                if !buffering || si < 4 {
                    if stored_moveto && stroke {
                        w.seg(Pe::Move(line_start));
                    }
                    stored_moveto = false;
                    if !do_close {
                        let mut i = si as usize;
                        while i > 0 && op_of(&segments[i - 1]) == 1 {
                            segments[i - 1] = segments[i];
                            i -= 1;
                        }
                        si = i as i32;
                        for s in segments.iter().take(si as usize) {
                            w.seg(*s);
                        }
                        si = 0;
                        let pe2 = next(&mut idx);
                        if let Some(pe2) = pe2 {
                            segments[0] = pe2;
                            w.seg(Pe::Close(P::default()));
                            if let Pe::Move(p) = pe2 {
                                buffering = matrix_optimisable;
                                si = 0;
                                initial_m = true;
                                line_start = p;
                            } else {
                                w.seg(pe2);
                                buffering = false;
                                si = -1;
                            }
                        } else {
                            // (the end after the closepath: dropped)
                            si = 0;
                            break;
                        }
                    } else {
                        for s in segments.iter().take(si as usize + 1) {
                            w.seg(*s);
                        }
                        buffering = matrix_optimisable;
                        si = -1;
                    }
                } else {
                    let n = si as usize;
                    let mut is_rect = true;
                    for i in 1..n {
                        let (a, b, c) =
                            (v0(&segments[i - 1]), v0(&segments[i]), v0(&segments[i + 1]));
                        if a.x != b.x {
                            if a.y != b.y {
                                is_rect = false;
                                break;
                            } else if b.x != c.x || b.y == c.x {
                                is_rect = false;
                                break;
                            }
                        } else if a.y == b.y {
                            is_rect = false;
                            break;
                        } else if b.y != c.y || b.x == c.x {
                            is_rect = false;
                            break;
                        }
                    }
                    let (s0, s1, sn) = (v0(&segments[0]), v0(&segments[1]), v0(&segments[n]));
                    if s0.x != sn.x || s0.y != sn.y {
                        is_rect = false;
                    }
                    if s0.x == s1.x && ty == PathType::DashedStroke {
                        is_rect = false;
                    }
                    if is_rect {
                        let s2 = v0(&segments[2]);
                        let (pt, width, height) = if s0.x == s1.x {
                            (s1, s2.x - s1.x, s0.y - s1.y)
                        } else {
                            (s0, s1.x - s0.x, s2.y - s1.y)
                        };
                        let (px, py) = scale_mat
                            .itransform(f64::from(pt.x), f64::from(pt.y))
                            .unwrap_or((0.0, 0.0));
                        let (qx, qy) = scale_mat
                            .itransform(f64::from(width), f64::from(height))
                            .unwrap_or((0.0, 0.0));
                        let (x0, y0) = (px as i32, py as i32);
                        let (x1, y1) = (x0 + qx as i32, y0 + qy as i32);
                        w.d.put(&format!(
                            "{} {} {} {} re\n",
                            pg(x2f(x0)),
                            pg(x2f(y0)),
                            pg(x2f(x1) - x2f(x0)),
                            pg(x2f(y1) - x2f(y0))
                        ));
                        si = -1;
                    } else {
                        for s in segments.iter().take(n + 1) {
                            w.seg(*s);
                        }
                        buffering = false;
                        si = -1;
                    }
                }
            }
        }
        si += 1;
        if si > 4 {
            for s in segments.iter().take(si as usize) {
                w.seg(*s);
            }
            si = 0;
            buffering = false;
        }
    }
    segs
}

/// Images and fonts keep their own resources; nothing more here.
pub type Shared<T> = Rc<T>;

#[allow(dead_code)]
fn _unused(_: Vec<u8>) {
    let _ = vec![0u8];
}
