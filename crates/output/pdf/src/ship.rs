//! Shipping a page or a form (pdfTeX §750–§771): the text state machine
//! (pdfTeX §691–§693), `pdf_hlist_out` and `pdf_vlist_out` (pdfTeX
//! §729–§746) with the whatsits (pdfTeX §1612–§1620), and the objects a
//! page or form writes after its contents.

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;
use phitex_typeset::glue::{GlueSign, Order};
use phitex_typeset::node::{BoxKind, BoxNode, GlueKind, GlueNode, Node, RUNNING};

use crate::num::{ONE_BP, ONE_HUNDRED_BP, TEN_POW, divide_scaled, round_xn_over_d};
use crate::{
    BILLION, ColorStackCmd, FontSource, FontState, FontType, LiteralMode, MAX_DIMEN, OBJ_TYPE_FONT,
    OBJ_TYPE_OTHERS, OBJ_TYPE_PAGE, OBJ_TYPE_PAGES, PAGES_TREE_KIDS_MAX, PdfError, PdfState,
    Whatsit, WhatsitSource, int, pdf_error, substr_of_str,
};

/// A box being output: what `pdf_hlist_out` and `pdf_vlist_out` keep of
/// `this_box` and their glue locals (pdfTeX §729, §738).
pub(crate) struct Frame {
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
    g_sign: GlueSign,
    g_order: Order,
    glue_set: f64,
    cur_g: Scaled,
    cur_glue: f64,
    /// `base_line` in an hlist, `top_edge` in a vlist.
    pub base: Scaled,
    pub left_edge: Scaled,
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

    /// The glue's size as set (pdfTeX §735, §744).
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

/// `vet_glue` then `round` (tex.web §625), in doubles as web2c computes.
#[allow(clippy::cast_possible_truncation)]
fn vet_round(glue_temp: f64) -> Scaled {
    let t = glue_temp.clamp(-BILLION, BILLION);
    if t >= 0.0 {
        (t + 0.5) as Scaled
    } else {
        (t - 0.5) as Scaled
    }
}

/// Pascal's `round` of `s/10`, halves away from zero.
fn round_tenth(s: i32) -> i32 {
    if s >= 0 { (s + 5) / 10 } else { (s - 5) / 10 }
}

impl PdfState {
    // ---- fonts on the page (pdfTeX §692–§693) ----

    pub(crate) fn font(&self, f: FontId) -> &FontState {
        &self.fonts[&f]
    }

    pub(crate) fn font_mut(&mut self, f: FontId) -> &mut FontState {
        self.fonts
            .get_mut(&f)
            .map(std::sync::Arc::make_mut)
            .expect("a font is loaded before it is used")
    }

    /// Whether font `f` is used (`font_used`); `null_font` is not.
    fn font_used(&self, f: FontId) -> bool {
        self.fonts.get(&f).is_some_and(|s| s.used)
    }

    /// `pdf_set_origin`.
    pub(crate) fn set_origin(&mut self, h: Scaled, v: Scaled) {
        if (h - self.origin_h).abs() >= self.min_bp_val
            || (v - self.origin_v).abs() >= self.min_bp_val
        {
            self.print(b"1 0 0 1 ");
            let so = self.print_bp(h - self.origin_h);
            self.origin_h += so;
            self.pdf_out(b' ');
            let so = self.print_bp(self.origin_v - v);
            self.origin_v -= so;
            self.print_ln(b" cm");
        }
        self.pdf_h = self.origin_h;
        self.tj_start_h = self.pdf_h;
        self.pdf_v = self.origin_v;
    }

    /// `pdf_set_origin_temp`.
    fn set_origin_temp(&mut self, h: Scaled, v: Scaled) {
        if (h - self.origin_h).abs() >= self.min_bp_val
            || (v - self.origin_v).abs() >= self.min_bp_val
        {
            self.print(b"1 0 0 1 ");
            self.print_bp(h - self.origin_h);
            self.pdf_out(b' ');
            self.print_bp(self.origin_v - v);
            self.print_ln(b" cm");
        }
    }

    /// `pdf_end_string`.
    fn end_string(&mut self) {
        if self.doing_string {
            self.print(b")]TJ");
            self.doing_string = false;
        }
    }

    /// `pdf_end_string_nl`.
    fn end_string_nl(&mut self) {
        if self.doing_string {
            self.print_ln(b")]TJ");
            self.doing_string = false;
        }
    }

    /// `pdf_set_text_pos`.
    fn set_text_pos(&mut self, v: i32, v_out: Scaled, f: FontId) {
        self.pdf_out(b' ');
        let new_tm_a = self.font(f).auto_expand_ratio();
        if new_tm_a != 0 || self.tm_a != 0 {
            self.print_real(1000 + new_tm_a, 3);
            self.print(b" 0 0 1 ");
            let so = self.print_bp(self.cur_h - self.origin_h);
            self.pdf_h = self.origin_h + so;
            self.pdf_out(b' ');
            let so = self.print_bp(self.origin_v - self.cur_v);
            self.pdf_v = self.origin_v - so;
            self.print(b" Tm");
            self.tm_a = new_tm_a;
        } else {
            let so = self.print_bp(self.cur_h - self.tj_start_h);
            self.pdf_h = self.tj_start_h + so;
            self.pdf_out(b' ');
            self.print_real(v, self.dd);
            self.pdf_v -= v_out;
            self.print(b" Td");
        }
        self.tj_start_h = self.pdf_h;
        self.delta_h = 0;
    }

    /// The font's state, from the caller's data the first time.
    pub(crate) fn load_font(&mut self, f: FontId, src: &dyn FontSource) {
        if self.fonts.contains_key(&f) {
            return;
        }
        let name = src.name(f).to_vec();
        self.map_read = true;
        let default = if self.no_default_map {
            None
        } else {
            src.map_entry(f)
        };
        let (map, own) = match self.map_over.get(&name) {
            Some(m) => (m.clone(), false),
            None => match default {
                Some(m) => (Some(m), true),
                None => (self.map_add.get(&name).cloned(), false),
            },
        };
        let file = map.as_ref().filter(|m| m.is_fontfile()).and_then(|m| {
            if own {
                src.font_file(f)
            } else {
                src.file(m.ff_name.as_deref().unwrap_or_default())
                    .or_else(|| src.font_file(f))
            }
        });
        let enc = map.as_ref().filter(|m| m.is_reencoded()).and_then(|m| {
            if own {
                src.encoding_file(f)
            } else {
                src.file(m.encname.as_deref().unwrap_or_default())
                    .or_else(|| src.encoding_file(f))
            }
        });
        self.rec_font(crate::replay::FontEv::Load(f));
        let file: Option<std::sync::Arc<[u8]>> = file.map(Into::into);
        let enc: Option<std::sync::Arc<[u8]>> = enc.map(Into::into);
        self.fonts.insert(
            f,
            std::sync::Arc::new(FontState {
                name,
                metrics: src.metrics(f).clone(),
                map,
                file,
                enc,
                used: false,
                size: 0,
                num: 0,
                chars: [false; 256],
                attr: Vec::new(),
                nobuiltin_tounicode: false,
                has_space: false,
                kind: FontType::New,
                vf: None,
                expand: src.expansion(f),
            }),
        );
    }

    /// `set_ff`.
    pub(crate) fn ff(&self, f: FontId) -> FontId {
        let n = self.font(f).num;
        if n < 0 {
            FontId::try_from(-n).unwrap_or(0)
        } else {
            f
        }
    }

    /// `pdf_use_font`.
    pub(crate) fn use_font(&mut self, f: FontId, num: i64) {
        let st = self.font_mut(f);
        st.size = divide_scaled(st.metrics.size, ONE_HUNDRED_BP, 6).1;
        st.used = true;
        st.num = num;
    }

    /// `hasspacechar`: a reencoded scalable font whose encoding has
    /// `space` at 32.
    fn has_space_char(&self, f: FontId) -> bool {
        let st = self.font(f);
        if !st.scalable()
            || !st
                .map
                .as_ref()
                .is_some_and(crate::map::MapEntry::is_reencoded)
        {
            return false;
        }
        st.enc
            .as_deref()
            .and_then(|e| crate::enc::load_enc(e).ok())
            .is_some_and(|g| g[32].as_deref() == Some(b"space"))
    }

    /// `pdf_init_font`.
    pub(crate) fn init_font(&mut self, f: FontId, src: &dyn FontSource) {
        self.rec_font(crate::replay::FontEv::Init(f));
        self.rec_depth(true);
        self.init_font_(f, src);
        self.rec_depth(false);
    }

    fn init_font_(&mut self, f: FontId, src: &dyn FontSource) {
        let x = self.font(f).expand.filter(|x| x.auto && x.base != 0);
        if let Some(x) = x {
            self.load_font(x.base, src);
            if !self.font(x.base).used {
                self.init_font(x.base, src);
            }
            let m = self.font(x.base).map.clone();
            self.font_mut(f).map = m;
        }
        if self.font(f).scalable() {
            let base_name = x.map(|x| self.font(x.base).name.clone());
            let mut i = self.head[OBJ_TYPE_FONT];
            while i != 0 {
                let k = FontId::try_from(self.objs[i].info).unwrap_or(0);
                let (fk, ff) = (self.font(k), self.font(f));
                if fk.scalable()
                    && fk.map == ff.map
                    && (fk.name == ff.name || base_name.as_ref() == Some(&fk.name))
                {
                    let n = fk.num;
                    if n < 0 {
                        self.use_font(f, n);
                    } else {
                        self.use_font(f, -i64::from(k));
                    }
                    return;
                }
                i = self.objs[i].link;
            }
        }
        let n = self.create_obj(OBJ_TYPE_FONT, i64::from(f));
        let space = self.has_space_char(f) && self.font(f).width(32) > ONE_BP;
        self.font_mut(f).has_space = space;
        self.use_font(f, int(n));
    }

    /// `pdf_set_font`.
    fn set_font(&mut self, f: FontId, src: &dyn FontSource) {
        self.pdf_f = f;
        if !self.font(f).used {
            self.init_font(f, src);
        }
        let k = self.ff(f);
        if !self.font_list.iter().any(|&g| self.ff(g) == k) {
            self.font_list.push(f);
        }
        let size = self.font(f).metrics.size;
        if k == self.last_f && size == self.last_fs {
            return;
        }
        self.print(b"/F");
        self.print_int(i64::from(k));
        self.pdf_out(b' ');
        let q = divide_scaled(size, ONE_HUNDRED_BP, 6).0;
        self.print_real(q, 4);
        self.print(b" Tf");
        self.last_f = k;
        self.last_fs = size;
    }

    /// `pdf_begin_text`.
    fn begin_text(&mut self) {
        self.set_origin(0, self.cur_page_height);
        self.print_ln(b"BT");
        self.doing_text = true;
        self.pdf_f = 0;
        self.last_f = 0;
        self.last_fs = 0;
        self.doing_string = false;
        self.tm_a = 0;
    }

    /// `pdf_read_dummy_font`.
    fn read_dummy_font(&mut self, src: &dyn FontSource) -> Option<FontId> {
        if self.dummy_font == 0 {
            let f = src.space_font()?;
            self.dummy_font = f;
            // pdfmaplinesp
            self.map_line(b"=pdftexspace PdfTeX-Space <pdftexspace.pfb");
            self.fonts.remove(&f);
            self.load_font(f, src);
            self.font_mut(f).chars[32] = true;
        }
        Some(self.dummy_font)
    }

    /// `pdf_insert_interword_space`.
    fn insert_interword_space(&mut self, src: &dyn FontSource) {
        let Some(d) = self.read_dummy_font(src) else {
            self.fail(pdf_error("font", "no \\pdfspacefont"));
            return;
        };
        self.set_font(d, src);
        self.print(b"( )Tj");
    }

    /// `pdf_begin_string`.
    fn begin_string(&mut self, f: FontId, src: &dyn FontSource) {
        let mut must_end_string = false;
        let mut must_insert_space = false;
        let mut must_set_text_pos = false;
        if !self.doing_text {
            self.begin_text();
            must_set_text_pos = true;
        }
        let faked = self.faked_space;
        let switch = if faked {
            !(self.font_used(f) && self.font_used(self.pdf_f))
        } else {
            self.pdf_f != f
        };
        if switch {
            self.end_string();
            self.set_font(f, src);
        }
        let size = self.font(f).size;
        let (mut s, mut s_out) = if self.tm_a == 0 {
            divide_scaled(self.cur_h - (self.tj_start_h + self.delta_h), size, 3)
        } else {
            let d = round_xn_over_d(
                self.cur_h - (self.tj_start_h + self.delta_h),
                1000,
                1000 + self.tm_a,
            );
            let (s, _) = divide_scaled(d, size, 3);
            let mut out = 0;
            if s.abs() < 0o100000 {
                out = round_xn_over_d(round_xn_over_d(size, s.abs(), 1000), 1000 + self.tm_a, 1000);
                if s < 0 {
                    out = -out;
                }
            }
            (s, out)
        };
        let (v, v_out) = if (self.cur_v - self.pdf_v).abs() >= self.min_bp_val {
            divide_scaled(self.pdf_v - self.cur_v, ONE_HUNDRED_BP, self.dd + 2)
        } else {
            (0, 0)
        };
        if !must_set_text_pos {
            let a = self.font(f).auto_expand_ratio();
            let b = self
                .fonts
                .get(&self.pdf_f)
                .map_or(0, |s| s.auto_expand_ratio());
            must_set_text_pos = v != 0 || s.abs() >= 0o100000 || a != self.tm_a || a != b;
        }
        if must_set_text_pos {
            must_end_string = true;
        }
        let m = &self.font(f).metrics;
        let (space, shrink) = (m.space(), m.space_shrink());
        let looks_like_space = space > ONE_BP && s_out > space - shrink - ONE_BP / 10 && v == 0;
        if faked && looks_like_space {
            must_insert_space = true;
        }
        if must_insert_space {
            if self.font(f).has_space && self.doing_string {
                self.pdf_out(b' ');
                let (ws, wout) = self.adv_char_width(f, 32);
                s -= ws;
                s_out -= wout;
                self.font_mut(f).chars[32] = true;
            } else {
                must_end_string = true;
            }
        }
        if must_end_string {
            self.end_string();
            if must_insert_space && !self.font(f).has_space {
                self.insert_interword_space(src);
            }
            self.set_font(f, src);
            self.set_text_pos(v, v_out, f);
            s = 0;
        }
        if faked && self.pdf_f != f {
            self.end_string();
            self.set_font(f, src);
        }
        if !self.doing_string {
            self.print(b" [");
            if s == 0 {
                self.pdf_out(b'(');
            }
        }
        if s != 0 {
            if self.doing_string {
                self.pdf_out(b')');
            }
            self.print_int(-i64::from(s));
            self.pdf_out(b'(');
            self.delta_h += s_out;
        }
        self.doing_string = true;
    }

    /// `pdf_insert_fake_space`.
    fn insert_fake_space(&mut self, src: &dyn FontSource) {
        let saved = self.faked_space;
        self.faked_space = false;
        if let Some(d) = self.read_dummy_font(src) {
            self.begin_string(d, src);
            self.print(b" ");
            self.adv_char_width(d, 32);
            self.end_string_nl();
        } else {
            self.fail(pdf_error("font", "no \\pdfspacefont"));
        }
        self.faked_space = saved;
    }

    /// `pdf_end_text`.
    pub(crate) fn end_text(&mut self) {
        if self.doing_text {
            self.end_string_nl();
            self.print_ln(b"ET");
            self.doing_text = false;
        }
    }

    /// `pdf_print_char`.
    fn print_char(&mut self, f: FontId, c: u8) {
        self.font_mut(f).chars[usize::from(c)] = true;
        if c <= 32 || c == 92 || c == 40 || c == 41 || c > 127 {
            self.pdf_out(b'\\');
            let o = format!("{c:03o}");
            self.print(o.as_bytes());
        } else {
            self.pdf_out(c);
        }
    }

    /// `adv_char_width`: (`adv_char_width_s`, `adv_char_width_s_out`).
    fn adv_char_width(&mut self, f: FontId, c: u16) -> (i32, Scaled) {
        let st = self.font(f);
        let w = st.width(c);
        let size = st.size;
        let (s, s_out) = if self.tm_a == 0 {
            divide_scaled(w, size, 4)
        } else {
            let (s, _) = divide_scaled(round_xn_over_d(w, 1000, 1000 + self.tm_a), size, 4);
            let mut out = round_xn_over_d(
                round_xn_over_d(size, s.abs(), 10000),
                1000 + self.tm_a,
                1000,
            );
            if s < 0 {
                out = -out;
            }
            (s, out)
        };
        self.delta_h += s_out;
        (round_tenth(s), s_out)
    }

    /// `pdf_set_rule` (pdfTeX §693).
    pub(crate) fn set_rule(&mut self, x: Scaled, y: Scaled, w: Scaled, h: Scaled) {
        self.end_text();
        self.print_ln(b"q");
        if h <= ONE_BP {
            self.set_origin_temp(x, y - (h + 1) / 2);
            self.print(b"[]0 d 0 J ");
            self.print_bp(h);
            self.print(b" w 0 0 m ");
            self.print_bp(w);
            self.print_ln(b" 0 l S");
        } else if w <= ONE_BP {
            self.set_origin_temp(x + (w + 1) / 2, y);
            self.print(b"[]0 d 0 J ");
            self.print_bp(w);
            self.print(b" w 0 0 m 0 ");
            self.print_bp(h);
            self.print_ln(b" l S");
        } else {
            self.set_origin_temp(x, y);
            self.print(b"0 0 ");
            self.print_bp(w);
            self.pdf_out(b' ');
            self.print_bp(h);
            self.print_ln(b" re f");
        }
        self.print_ln(b"Q");
    }

    /// `literal(s, literal_mode, warn)` (pdfTeX §693).
    pub(crate) fn literal(&mut self, s: &[u8], mode: LiteralMode) {
        let (mode, body) = if mode == LiteralMode::ScanSpecial {
            let Some(rest) = s.strip_prefix(b"PDF:").or_else(|| s.strip_prefix(b"pdf:")) else {
                // "Non-PDF special ignored!" goes to the log
                return;
            };
            if let Some(b) = rest.strip_prefix(b"direct:") {
                (LiteralMode::DirectAlways, b)
            } else if let Some(b) = rest.strip_prefix(b"page:") {
                (LiteralMode::DirectPage, b)
            } else {
                (LiteralMode::SetOrigin, rest)
            }
        } else {
            (mode, s)
        };
        match mode {
            LiteralMode::SetOrigin => {
                self.end_text();
                self.set_origin(self.cur_h, self.cur_v);
            }
            LiteralMode::DirectPage => self.end_text(),
            _ => self.end_string_nl(),
        }
        self.print(body);
        self.pdf_out(b'\n');
    }

    /// `pdf_out_colorstack`.
    fn out_colorstack(&mut self, stack: usize, cmd: &ColorStackCmd) {
        if stack >= self.stacks.used() {
            // "Color stack n is not initialized for use!"
            return;
        }
        let (mode, s) = match cmd {
            ColorStackCmd::Set(d) => (self.stacks.set(stack, d), d.clone()),
            ColorStackCmd::Push(d) => (self.stacks.push(stack, d), d.clone()),
            ColorStackCmd::Pop => self.stacks.pop(stack),
            ColorStackCmd::Current => self.stacks.current(stack),
        };
        if !s.is_empty() {
            self.literal(&s, mode);
        }
    }

    /// `pdf_out_colorstack_startpage`.
    fn out_colorstack_startpage(&mut self) {
        for i in 0..self.stacks.used() {
            if self.stacks.skip_page_start(i) == 0 {
                let (mode, s) = self.stacks.current(i);
                if !s.is_empty() {
                    self.literal(&s, mode);
                }
            }
        }
    }

    /// `pdf_out_setmatrix`.
    fn out_setmatrix(&mut self, data: &[u8]) {
        let (h, v) = (self.cur_h, self.cur_page_height - self.cur_v);
        if self.stacks.set_matrix(data, h, v) {
            let mut s = data.to_vec();
            s.extend_from_slice(b" 0 0 cm");
            self.literal(&s, LiteralMode::SetOrigin);
        } else {
            self.fail(pdf_error("\\pdfsetmatrix", "Unrecognized format."));
        }
    }

    // ---- lists (pdfTeX §729–§746) ----

    fn box_out(&mut self, b: &BoxNode, src: &dyn FontSource, wh: &mut WhatsitSource<'_>) {
        match b.kind {
            BoxKind::H => self.hlist_out(b, src, wh),
            BoxKind::V => self.vlist_out(b, src, wh),
        }
    }

    /// `output_one_char` (pdfTeX §726 with `do_vf`).
    pub(crate) fn output_one_char(&mut self, f: FontId, c: u8, src: &dyn FontSource) {
        if self.font(f).kind == FontType::New {
            self.do_vf(f, src);
        }
        if self.font(f).kind == FontType::Virtual {
            self.do_vf_packet(f, c, src);
        } else {
            self.begin_string(f, src);
            self.print_char(f, c);
            self.adv_char_width(f, u16::from(c));
        }
    }

    /// A character in an hlist (pdfTeX §731).
    fn char_out(&mut self, f: FontId, c: u16, src: &dyn FontSource) {
        self.load_font(f, src);
        if self.font(f).valid(c) {
            self.output_one_char(f, u8::try_from(c).unwrap_or(0), src);
        }
        self.cur_h += self.font(f).width(c);
    }

    /// `pdf_hlist_out` (pdfTeX §729).
    fn hlist_out(&mut self, b: &BoxNode, src: &dyn FontSource, wh: &mut WhatsitSource<'_>) {
        self.cur_s += 1;
        let mut fr = Frame::new(b, self.cur_v, self.cur_h);
        // "Create link annotations for the current hbox if needed"
        for i in 0..self.link_stack.len() {
            if self.link_stack[i].level == self.cur_s && self.running_link {
                self.append_link(&fr, fr.left_edge, fr.base, i);
            }
        }
        self.hlist_nodes(&b.list, &mut fr, src, wh);
        self.cur_s -= 1;
    }

    /// The nodes of an hlist (pdfTeX §731–§737); a discretionary's
    /// replacement text is part of the list.
    fn hlist_nodes(
        &mut self,
        list: &[Node],
        fr: &mut Frame,
        src: &dyn FontSource,
        wh: &mut WhatsitSource<'_>,
    ) {
        for p in list {
            let rule = match p {
                Node::Char { font, ch } => {
                    self.char_out(*font, *ch, src);
                    None
                }
                Node::Lig(l) => {
                    self.char_out(l.font, l.ch, src);
                    None
                }
                Node::Box(bx) => {
                    // §733
                    if bx.list.is_empty() {
                        self.cur_h += bx.width;
                    } else {
                        self.cur_v = fr.base + bx.shift;
                        let edge = self.cur_h + bx.width;
                        self.box_out(bx, src, wh);
                        self.cur_h = edge;
                        self.cur_v = fr.base;
                    }
                    None
                }
                Node::Rule(r) => Some((r.height, r.depth, r.width)),
                Node::Whatsit(h, size) => {
                    if let Some(w) = wh(*h, self.doing_leaders) {
                        self.whatsit(&w, fr, false, src, wh);
                    }
                    // "Output a Form/Image node in a hlist":
                    // `cur_h:=edge+pdf_width(p)`
                    if let Some(s) = size {
                        self.cur_h += s.width;
                    }
                    None
                }
                Node::Glue(g) => self.hglue(g, fr, src, wh),
                Node::Kern(w, _) | Node::Math(_, w) => {
                    self.cur_h += w;
                    None
                }
                Node::Disc(d) => {
                    self.hlist_nodes(&d.replace, fr, src, wh);
                    None
                }
                Node::Unset(_)
                | Node::Ins(_)
                | Node::Mark(_)
                | Node::Adjust(_)
                | Node::Language { .. }
                | Node::Penalty(_) => None,
            };
            if let Some((mut rule_ht, mut rule_dp, rule_wd)) = rule {
                // §734
                if rule_ht == RUNNING {
                    rule_ht = fr.height;
                }
                if rule_dp == RUNNING {
                    rule_dp = fr.depth;
                }
                rule_ht += rule_dp;
                if rule_ht > 0 && rule_wd > 0 {
                    self.cur_v = fr.base + rule_dp;
                    self.set_rule(self.cur_h, self.cur_v, rule_wd, rule_ht);
                    self.cur_v = fr.base;
                }
                self.cur_h += rule_wd;
            }
        }
    }

    /// Glue in an hlist (pdfTeX §735–§737): the rule to output, or `None`
    /// once `cur_h` has moved past it.
    fn hglue(
        &mut self,
        g: &GlueNode,
        fr: &mut Frame,
        src: &dyn FontSource,
        wh: &mut WhatsitSource<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        let mut rule_wd = fr.glue(g);
        if matches!(
            g.kind,
            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
        ) {
            match g.leader.as_deref() {
                Some(Node::Rule(r)) => return Some((r.height, r.depth, rule_wd)),
                Some(Node::Box(lb)) => {
                    let leader_wd = lb.width;
                    if leader_wd > 0 && rule_wd > 0 {
                        rule_wd += 10;
                        let edge = self.cur_h + rule_wd;
                        let mut lx = 0;
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
                            // §737
                            self.cur_v = fr.base + lb.shift;
                            let save_h = self.cur_h;
                            let outer = self.doing_leaders;
                            self.doing_leaders = true;
                            self.box_out(lb, src, wh);
                            self.doing_leaders = outer;
                            self.cur_v = fr.base;
                            self.cur_h = save_h + leader_wd + lx;
                        }
                        self.cur_h = edge - 10;
                        return None;
                    }
                }
                _ => {}
            }
        }
        self.cur_h += rule_wd;
        None
    }

    /// `pdf_vlist_out` (pdfTeX §738–§746).
    fn vlist_out(&mut self, b: &BoxNode, src: &dyn FontSource, wh: &mut WhatsitSource<'_>) {
        self.cur_s += 1;
        let left_edge = self.cur_h;
        self.cur_v -= b.height;
        let mut fr = Frame::new(b, self.cur_v, left_edge);
        // "Create thread for the current vbox if needed"
        if self.thread.last.is_some()
            && self.thread.dp == RUNNING
            && self.thread.level == self.cur_s
        {
            self.append_thread(&fr, left_edge, fr.base + fr.height);
        }
        for p in &b.list {
            let rule = match p {
                Node::Char { .. } | Node::Lig(_) => {
                    self.fail(pdf_error("", "This can't happen (pdfvlistout)"));
                    None
                }
                Node::Box(bx) => {
                    // §742
                    if bx.list.is_empty() {
                        self.cur_v += bx.height + bx.depth;
                    } else {
                        self.cur_v += bx.height;
                        let save_v = self.cur_v;
                        self.cur_h = left_edge + bx.shift;
                        self.box_out(bx, src, wh);
                        self.cur_v = save_v + bx.depth;
                        self.cur_h = left_edge;
                    }
                    None
                }
                Node::Rule(r) => Some((r.height, r.depth, r.width)),
                Node::Whatsit(h, size) => {
                    if let Some(w) = wh(*h, self.doing_leaders) {
                        self.whatsit(&w, &fr, true, src, wh);
                    }
                    // "Output a Form/Image node in a vlist":
                    // `cur_v:=save_v+pdf_depth(p)` after its height
                    if let Some(s) = size {
                        self.cur_v += s.height + s.depth;
                    }
                    None
                }
                Node::Glue(g) => self.vglue(g, &mut fr, src, wh),
                Node::Kern(w, _) => {
                    self.cur_v += w;
                    None
                }
                _ => None,
            };
            if let Some((rule_ht, rule_dp, mut rule_wd)) = rule {
                // §743
                if rule_wd == RUNNING {
                    rule_wd = fr.width;
                }
                let rule_ht = rule_ht + rule_dp;
                self.cur_v += rule_ht;
                if rule_ht > 0 && rule_wd > 0 {
                    self.set_rule(self.cur_h, self.cur_v, rule_wd, rule_ht);
                    self.cur_h = left_edge;
                }
            }
        }
        self.cur_s -= 1;
    }

    /// Glue in a vlist (pdfTeX §744–§746).
    fn vglue(
        &mut self,
        g: &GlueNode,
        fr: &mut Frame,
        src: &dyn FontSource,
        wh: &mut WhatsitSource<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        let mut rule_ht = fr.glue(g);
        if matches!(
            g.kind,
            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
        ) {
            match g.leader.as_deref() {
                Some(Node::Rule(r)) => return Some((rule_ht, 0, r.width)),
                Some(Node::Box(lb)) => {
                    let leader_ht = lb.height + lb.depth;
                    if leader_ht > 0 && rule_ht > 0 {
                        rule_ht += 10;
                        let edge = self.cur_v + rule_ht;
                        let mut lx = 0;
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
                            // §746
                            self.cur_h = fr.left_edge + lb.shift;
                            self.cur_v += lb.height;
                            let save_v = self.cur_v;
                            let outer = self.doing_leaders;
                            self.doing_leaders = true;
                            self.box_out(lb, src, wh);
                            self.doing_leaders = outer;
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
        self.cur_v += rule_ht;
        None
    }

    /// "Output the whatsit node p in `pdf_hlist_out`" and "in
    /// `pdf_vlist_out`" (pdfTeX §1612–§1620). In a vlist, (x, y) is
    /// (`left_edge`, `top_edge+height(this_box)`); in an hlist,
    /// (`left_edge`, `base_line`).
    #[allow(clippy::too_many_lines)]
    fn whatsit(
        &mut self,
        w: &Whatsit,
        fr: &Frame,
        vertical: bool,
        src: &dyn FontSource,
        _wh: &mut WhatsitSource<'_>,
    ) {
        let (x, y) = if vertical {
            (fr.left_edge, fr.base + fr.height)
        } else {
            (fr.left_edge, fr.base)
        };
        match w {
            Whatsit::Special(s) => self.literal(s, LiteralMode::ScanSpecial),
            Whatsit::Literal(mode, s) => self.literal(s, *mode),
            Whatsit::ColorStack(n, cmd) => self.out_colorstack(*n, cmd),
            Whatsit::SetMatrix(d) => self.out_setmatrix(d),
            Whatsit::Save => {
                self.stacks.save(self.cur_h, self.cur_v);
                self.literal(b"q", LiteralMode::SetOrigin);
            }
            Whatsit::Restore => {
                self.stacks.restore();
                self.literal(b"Q", LiteralMode::SetOrigin);
            }
            Whatsit::RefObj(n) => {
                // pdfTeX §1383 (`scan_refobj`): a number that is not an
                // object of this file, as for a form or an image
                if *n == 0 || *n > self.sys_obj_ptr() {
                    self.fail(pdf_error("ext1", "cannot find referenced object"));
                    return;
                }
                self.obj_list.push(*n);
            }
            Whatsit::RefXForm(n) => {
                let Some((_, h, _)) = self.xform_size(*n) else {
                    self.fail(pdf_error("ext1", "cannot find referenced object"));
                    return;
                };
                let (save_h, save_v) = (self.cur_h, self.cur_v);
                if vertical {
                    // "Output a Form node in a vlist"
                    self.cur_v += h;
                    self.cur_h = x;
                } else {
                    self.cur_v = y;
                }
                self.out_form(*n);
                self.cur_h = save_h;
                self.cur_v = save_v;
            }
            Whatsit::RefXImage(n) => {
                let Some(info) = self.ximage_info(*n) else {
                    self.fail(pdf_error("ext1", "cannot find referenced object"));
                    return;
                };
                let (save_h, save_v) = (self.cur_h, self.cur_v);
                if vertical {
                    self.cur_v += info.height + info.depth;
                    self.cur_h = x;
                } else {
                    self.cur_v = y + info.depth;
                }
                self.out_image(*n, info.width, info.height, info.depth);
                self.cur_h = save_h;
                self.cur_v = save_v;
            }
            Whatsit::Annot {
                objnum,
                width,
                height,
                depth,
                data,
            } => self.do_annot(*objnum, [*width, *height, *depth], data, fr, x, y),
            Whatsit::StartLink {
                objnum,
                width,
                height,
                depth,
                attr,
                action,
            } => {
                if vertical {
                    self.fail(pdf_error("ext4", "\\pdfstartlink ended up in vlist"));
                } else {
                    self.do_link(
                        *objnum,
                        [*width, *height, *depth],
                        attr.as_ref(),
                        action,
                        fr,
                        x,
                        y,
                    );
                }
            }
            Whatsit::EndLink => {
                if vertical {
                    self.fail(pdf_error("ext4", "\\pdfendlink ended up in vlist"));
                } else {
                    self.end_link();
                }
            }
            Whatsit::Dest {
                id,
                struct_num,
                kind,
            } => self.do_dest(id, *struct_num, *kind, fr, x, y),
            Whatsit::Thread {
                start,
                id,
                width,
                height,
                depth,
                attr,
            } => {
                if *start && !vertical {
                    self.fail(pdf_error("ext4", "\\pdfstartthread ended up in hlist"));
                } else {
                    self.do_thread(
                        *start,
                        id,
                        [*width, *height, *depth],
                        attr.as_ref(),
                        fr,
                        x,
                        y,
                    );
                }
            }
            Whatsit::EndThread => {
                if vertical {
                    self.end_thread();
                } else {
                    self.fail(pdf_error("ext4", "\\pdfendthread ended up in hlist"));
                }
            }
            Whatsit::SavePos => {}
            Whatsit::InterwordSpaceOn => self.faked_space = true,
            Whatsit::InterwordSpaceOff => self.faked_space = false,
            Whatsit::FakeSpace => self.insert_fake_space(src),
            Whatsit::RunningLinkOff => self.running_link = false,
            Whatsit::RunningLinkOn => self.running_link = true,
        }
    }

    /// `out_form` (pdfTeX §1619).
    fn out_form(&mut self, n: usize) {
        self.end_text();
        self.print_ln(b"q");
        if !self.xform_list.contains(&n) {
            self.xform_list.push(n);
        }
        let depth = self.aux.xforms.get(&n).map_or(0, |x| x.depth);
        self.cur_v += depth;
        self.print(b"1 0 0 1 ");
        self.print_bp(self.cur_h - self.origin_h);
        self.pdf_out(b' ');
        self.print_bp(self.origin_v - self.cur_v);
        self.print_ln(b" cm");
        self.print(b"/Fm");
        self.print_int(self.objs[n].info);
        self.print_ln(b" Do");
        self.print_ln(b"Q");
    }

    // ---- shipping out (pdfTeX §750–§771) ----

    /// `pdf_ship_out(p, true)`: one page, for lists whose whatsits are
    /// only `\special`s; `whatsits` answers a whatsit with its text.
    ///
    /// # Errors
    /// [`PdfError::HugePage`] for a page TeX refuses; nothing is written.
    pub fn ship_out(
        &mut self,
        page: &crate::BoxNode,
        fonts: &dyn FontSource,
        whatsits: &mut crate::Whatsits<'_>,
    ) -> Result<(), PdfError> {
        let mut wh = |h: u32, leaders: bool| whatsits(h, leaders).map(Whatsit::Special);
        self.ship(page, true, fonts, &mut wh)
    }

    /// `pdf_ship_out(p, true)`: one page; `whatsits` answers each
    /// whatsit with its data.
    ///
    /// # Errors
    /// [`PdfError::HugePage`] for a page TeX refuses (nothing is
    /// written), or what pdfTeX stops at (`pdf_error`, `pdftex_fail`).
    pub fn ship_out_with(
        &mut self,
        page: &crate::BoxNode,
        fonts: &dyn FontSource,
        whatsits: &mut WhatsitSource<'_>,
    ) -> Result<(), PdfError> {
        self.ship(page, true, fonts, whatsits)
    }

    /// "Initialize variables for PDF output" (pdfTeX §792).
    pub(crate) fn init_output(&mut self) {
        if !self.initialized {
            self.check_pdfversion();
            self.dd = self.p.decimal_digits.clamp(0, 4);
            let ten = TEN_POW[usize::try_from(self.dd + 2).unwrap_or(0)];
            self.min_bp_val = divide_scaled(ONE_HUNDRED_BP, i32::try_from(ten).unwrap_or(1), 0).0;
            self.initialized = true;
        }
    }

    /// `pdf_ship_out(p, shipping_page)` (pdfTeX §750–§771).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn ship(
        &mut self,
        p: &crate::BoxNode,
        shipping_page: bool,
        src: &dyn FontSource,
        wh: &mut WhatsitSource<'_>,
    ) -> Result<(), PdfError> {
        self.init_output();
        self.shipping_page = shipping_page;
        // §751: tex.web §641's test
        let too_big = |a: Scaled, b: Scaled| a.checked_add(b).is_none_or(|s| s > MAX_DIMEN);
        if p.height > MAX_DIMEN
            || p.depth > MAX_DIMEN
            || p.height
                .checked_add(p.depth)
                .is_none_or(|hd| too_big(hd, self.p.v_offset))
            || too_big(p.width, self.p.h_offset)
        {
            return Err(PdfError::HugePage);
        }
        // §752
        let last_resources = self.new_objnum_();
        self.page_group_val = 0;
        let saved = self.reset_resource_lists();
        let last_stream;
        if shipping_page {
            let cur_h_offset = self.p.h_origin + self.p.h_offset;
            let cur_v_offset = self.p.v_origin + self.p.v_offset;
            self.cur_page_width = if self.p.page_width != 0 {
                self.p.page_width
            } else {
                p.width + 2 * cur_h_offset
            };
            self.cur_page_height = if self.p.page_height != 0 {
                self.p.page_height
            } else {
                p.height + p.depth + 2 * cur_v_offset
            };
            self.last_page = self.get_obj(
                OBJ_TYPE_PAGE,
                &crate::Ident::Num(i32::try_from(self.total_pages + 1).unwrap_or(i32::MAX)),
            );
            self.aux.pages_made.insert(self.last_page);
            last_stream = self.new_dict(OBJ_TYPE_OTHERS, 0, 0);
            if let Some(r) = &mut self.rec {
                r.blob = Some((self.out.len(), 0));
                r.pre_nums = [last_resources, self.last_page, last_stream];
                r.walk = true;
            }
            self.cur_h = cur_h_offset;
            self.cur_v = p.height + cur_v_offset;
            self.origin_h = 0;
            self.origin_v = self.cur_page_height;
            // "Reset PDF mark lists"
            self.annot_list.clear();
            self.link_list.clear();
            self.dest_list.clear();
            self.bead_list.clear();
            self.thread.last = None;
        } else {
            let form = self.cur_form;
            self.xform_height = p.height;
            self.xform_depth = p.depth;
            self.begin_dict(form, 0);
            last_stream = form;
            self.cur_v = p.height;
            self.cur_h = 0;
            self.origin_h = 0;
            self.origin_v = p.height + p.depth;
            // "Write out Form stream header"
            self.print_ln(b"/Type /XObject");
            self.print_ln(b"/Subtype /Form");
            let attr = self.aux.xforms.get_mut(&form).and_then(|x| x.attr.take());
            if let Some(a) = attr {
                self.print_toks_ln(&a);
            }
            self.print(b"/BBox [");
            self.print(b"0 0 ");
            self.print_bp(p.width);
            self.pdf_out(b' ');
            self.print_bp(p.height + p.depth);
            self.print_ln(b"]");
            self.print_ln(b"/FormType 1");
            self.print_ln(b"/Matrix [1 0 0 1 0 0]");
            self.indirect_ln(b"Resources", last_resources);
        }
        // "Start stream of page/form contents"
        self.begin_stream();
        if shipping_page && self.p.mag != 1000 {
            self.print_real(self.p.mag, 3);
            self.print(b" 0 0 ");
            self.print_real(self.p.mag, 3);
            self.print_ln(b" 0 0 cm");
        }
        self.stacks.ship_begin(shipping_page);
        if shipping_page {
            self.out_colorstack_startpage();
        }
        self.box_out(p, src, wh);
        if shipping_page {
            self.total_pages += 1;
        }
        self.cur_s = -1;
        // "Finish stream of page/form contents"
        self.end_text();
        if self.stacks.unmatched_saves() > 0 {
            let what = if shipping_page { "page" } else { "form" };
            self.fail(pdf_error(
                "",
                &format!(
                    "{} unmatched \\pdfsave after {what} shipout",
                    self.stacks.unmatched_saves()
                ),
            ));
        }
        if shipping_page {
            let data = match &self.zip {
                Some(z) => z.clone(),
                None => self.out[self.save_offset - self.base..].to_vec(),
            };
            self.page_shown = (data, self.cur_page_width, self.cur_page_height);
        }
        self.end_stream();
        if shipping_page && let Some(r) = &mut self.rec {
            if let Some(b) = &mut r.blob {
                b.1 = self.out.len();
            }
            r.walk = false;
            r.font_list.clone_from(&self.font_list);
            r.other = !self.xform_list.is_empty()
                || !self.bead_list.is_empty()
                || self.page_group_val != 0;
            r.obj_list.clone_from(&self.obj_list);
            r.ximage_list.clone_from(&self.ximage_list);
            r.marks = self
                .annot_list
                .iter()
                .chain(&self.link_list)
                .filter_map(|k| Some((*k, self.aux.marks.get(k)?.clone())))
                .collect();
            r.dest_data = self
                .dest_list
                .iter()
                .filter_map(|k| Some((*k, self.aux.dests.get(k)?.clone())))
                .collect();
        }
        if shipping_page {
            self.write_page_object(last_stream, last_resources);
        }
        // "Write out resource lists"
        for k in self.obj_list.clone() {
            if !self.is_written(k) {
                self.pdf_write_obj(k);
            }
        }
        for k in self.ximage_list.clone() {
            if !self.is_written(k)
                && let Err(e) = self.pdf_write_image(k, src)
            {
                self.fail(e);
            }
        }
        for k in self.xform_list.clone() {
            if !self.is_written(k) {
                self.ship_form(k, src, wh);
            }
        }
        if shipping_page {
            self.write_page_marks();
        }
        self.write_resources(last_resources, shipping_page);
        self.restore_resource_lists(saved);
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// "Reset resource lists", returning the old ones (pdfTeX saves them
    /// only around forms; a page starts with them empty either way).
    #[allow(clippy::type_complexity)]
    pub(crate) fn reset_resource_lists(
        &mut self,
    ) -> (Vec<FontId>, Vec<usize>, Vec<usize>, Vec<usize>, bool, i32) {
        (
            std::mem::take(&mut self.font_list),
            std::mem::take(&mut self.obj_list),
            std::mem::take(&mut self.xform_list),
            std::mem::take(&mut self.ximage_list),
            std::mem::replace(&mut self.text_procset, false),
            std::mem::replace(&mut self.image_procset, 0),
        )
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn restore_resource_lists(
        &mut self,
        l: (Vec<FontId>, Vec<usize>, Vec<usize>, Vec<usize>, bool, i32),
    ) {
        (
            self.font_list,
            self.obj_list,
            self.xform_list,
            self.ximage_list,
            self.text_procset,
            self.image_procset,
        ) = l;
    }

    /// "Write out pending forms": ship form `k` within the page.
    fn ship_form(&mut self, k: usize, src: &dyn FontSource, wh: &mut WhatsitSource<'_>) {
        let saved_form = self.cur_form;
        self.cur_form = k;
        let Some(b) = self.aux.xforms.get_mut(&k).and_then(|x| x.boxed.take()) else {
            return;
        };
        let keep = (
            self.shipping_page,
            self.cur_h,
            self.cur_v,
            self.origin_h,
            self.origin_v,
            self.xform_height,
            self.xform_depth,
        );
        let err = self.error.take();
        if let Err(e) = self.ship(&b, false, src, wh) {
            self.fail(e);
        }
        if let Some(e) = err {
            self.error = Some(e);
        }
        (
            self.shipping_page,
            self.cur_h,
            self.cur_v,
            self.origin_h,
            self.origin_v,
            self.xform_height,
            self.xform_depth,
        ) = keep;
        self.cur_form = saved_form;
    }

    /// "Write out page object" (pdfTeX §769).
    pub(crate) fn write_page_object(&mut self, last_stream: usize, last_resources: usize) {
        let page = self.last_page;
        self.begin_dict(page, 1);
        self.print_ln(b"/Type /Page");
        self.indirect_ln(b"Contents", last_stream);
        self.indirect_ln(b"Resources", last_resources);
        let attr = self.p.page_attr.clone();
        let mediabox_given = attr
            .as_ref()
            .is_some_and(|s| substr_of_str(b"/MediaBox", s));
        if !mediabox_given {
            self.print(b"/MediaBox [0 0 ");
            self.print_mag_bp(self.cur_page_width);
            self.pdf_out(b' ');
            self.print_mag_bp(self.cur_page_height);
            self.print_ln(b"]");
        }
        if let Some(a) = attr {
            self.print_toks_ln(&a);
        }
        // "Generate parent pages object"
        if self.total_pages % PAGES_TREE_KIDS_MAX == 1 {
            self.last_pages = self.create_obj(OBJ_TYPE_PAGES, PAGES_TREE_KIDS_MAX);
        }
        self.indirect_ln(b"Parent", self.last_pages);
        if self.page_group_val > 0 {
            self.print(b"/Group ");
            self.print_int(int(self.page_group_val));
            self.print_ln(b" 0 R");
        }
        // "Generate array of annotations or beads in page"
        if !self.annot_list.is_empty() || !self.link_list.is_empty() {
            self.print(b"/Annots [ ");
            for k in self
                .annot_list
                .clone()
                .into_iter()
                .chain(self.link_list.clone())
            {
                self.print_int(int(k));
                self.print(b" 0 R ");
            }
            self.print_ln(b"]");
        }
        if !self.bead_list.is_empty() {
            self.print(b"/B [ ");
            for k in self.bead_list.clone() {
                self.print_int(int(k));
                self.print(b" 0 R ");
            }
            self.print_ln(b"]");
        }
        self.end_dict();
    }

    /// "Write out resources dictionary" (pdfTeX §762–§768).
    pub(crate) fn write_resources(&mut self, last_resources: usize, shipping_page: bool) {
        self.begin_dict(last_resources, 1);
        if shipping_page {
            if let Some(r) = self.p.page_resources.clone() {
                self.print_toks_ln(&r);
            }
        } else {
            let form = self.cur_form;
            let r = self
                .aux
                .xforms
                .get_mut(&form)
                .and_then(|x| x.resources.take());
            if let Some(r) = r {
                self.print_toks_ln(&r);
            }
        }
        if !self.font_list.is_empty() {
            self.print(b"/Font << ");
            for f in self.font_list.clone() {
                let k = self.ff(f);
                self.print(b"/F");
                self.print_int(i64::from(k));
                self.pdf_out(b' ');
                self.print_int(self.font(k).num);
                self.print(b" 0 R ");
            }
            self.print_ln(b">>");
            self.text_procset = true;
        }
        if !self.xform_list.is_empty() || !self.ximage_list.is_empty() {
            self.print(b"/XObject << ");
            for k in self.xform_list.clone() {
                self.print(b"/Fm");
                self.print_int(self.objs[k].info);
                self.pdf_out(b' ');
                self.print_int(int(k));
                self.print(b" 0 R ");
            }
            for k in self.ximage_list.clone() {
                self.print(b"/Im");
                self.print_int(self.objs[k].info);
                self.pdf_out(b' ');
                self.print_int(int(k));
                self.print(b" 0 R ");
                self.image_procset |= self.aux.ximages.get(&k).map_or(0, |x| x.color);
            }
            self.print_ln(b">>");
        }
        if self.p.omit_procset < 0 || (self.p.omit_procset == 0 && self.p.major_version < 2) {
            self.print(b"/ProcSet [ /PDF");
            if self.text_procset {
                self.print(b" /Text");
            }
            if self.image_procset & crate::image::IMAGE_COLOR_B != 0 {
                self.print(b" /ImageB");
            }
            if self.image_procset & crate::image::IMAGE_COLOR_C != 0 {
                self.print(b" /ImageC");
            }
            if self.image_procset & crate::image::IMAGE_COLOR_I != 0 {
                self.print(b" /ImageI");
            }
            self.print_ln(b" ]");
        }
        self.end_dict();
    }
}
