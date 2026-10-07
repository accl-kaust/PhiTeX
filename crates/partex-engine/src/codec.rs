//! A compact binary form of nodes, for format files (and caches).
//!
//! Little-endian, self-delimiting, no version of its own: the container
//! (a format file) carries one. Decoding returns `None` on malformed input
//! instead of panicking.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::Scaled;
use crate::node::{Action, Dims, PdfId, PdfWhatsit};
use crate::node::{
    Adjust, BoxNode, Disc, FontId, GlueSign, GlueSpec, Glyphs, Ins, LeaderNode, Leaders, Ligature,
    Mark, Node, Order, Unset, Whatsit,
};

/// An encoder.
#[derive(Clone, Debug, Default)]
pub struct Enc(pub Vec<u8>);

impl Enc {
    pub fn u8(&mut self, x: u8) {
        self.0.push(x);
    }
    pub fn i32(&mut self, x: i32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    /// A length or count.
    ///
    /// # Panics
    ///
    /// If `n` does not fit an `i32`.
    pub fn count(&mut self, n: usize) {
        self.i32(i32::try_from(n).expect("encoded length"));
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.count(b.len());
        self.0.extend_from_slice(b);
    }
    pub fn ints(&mut self, xs: &[i32]) {
        self.count(xs.len());
        for &x in xs {
            self.i32(x);
        }
    }
    fn order(&mut self, o: Order) {
        self.u8(u8::try_from(o.index()).unwrap_or(0));
    }
    pub fn glue(&mut self, g: &GlueSpec) {
        self.i32(g.width);
        self.i32(g.stretch);
        self.i32(g.shrink);
        self.order(g.stretch_order);
        self.order(g.shrink_order);
        self.u8(u8::from(g.shared_zero));
    }
    pub fn box_node(&mut self, b: &BoxNode) {
        self.u8(u8::from(b.vertical));
        for x in [b.width, b.height, b.depth, b.shift] {
            self.i32(x);
        }
        self.0
            .extend_from_slice(&b.glue_set.to_bits().to_le_bytes());
        // (an inline `SyncTeX` place: the sign's top bit, then the place)
        let sign = match b.glue_sign {
            GlueSign::Normal => 0,
            GlueSign::Stretching => 1,
            GlueSign::Shrinking => 2,
        };
        if b.sync.is_inline() {
            self.u8(sign | 0x80);
            self.i32(b.sync.0.cast_signed());
        } else {
            self.u8(sign);
        }
        self.order(b.glue_order);
        // (a sealed box: the subtype's top bit, then the key)
        match b.seal {
            None => self.u8(b.subtype),
            Some(k) => {
                self.u8(b.subtype | 0x80);
                self.0.extend_from_slice(&k.to_le_bytes());
            }
        }
        self.list(&b.list);
    }
    pub fn list(&mut self, list: &[Node]) {
        self.count(list.len());
        for n in list {
            self.node(n);
        }
    }
    pub fn node(&mut self, n: &Node) {
        // (an inline `SyncTeX` place of a rule, glue, leaders, kern, math
        // or unset node: tag 16 and the place before the node; a box's is
        // in its own encoding)
        if let Some(sync) = inline_place(n) {
            self.u8(16);
            self.i32(sync.0.cast_signed());
        }
        match n {
            Node::Glyphs(g) => {
                self.u8(0);
                self.i32(i32::from(g.font.0));
                self.bytes(g.chars());
            }
            Node::Box(b) => {
                self.u8(1);
                self.box_node(b);
            }
            Node::Rule {
                width,
                height,
                depth,
                ..
            } => {
                self.u8(2);
                self.i32(*width);
                self.i32(*height);
                self.i32(*depth);
            }
            Node::Glue { spec, subtype, .. } => {
                self.u8(3);
                self.glue(spec);
                self.u8(*subtype);
            }
            Node::Leaders(l) => {
                self.u8(4);
                self.glue(&l.spec);
                self.u8(match l.kind {
                    Leaders::Aligned => 0,
                    Leaders::Centered => 1,
                    Leaders::Expanded => 2,
                });
                self.node(&l.leader);
            }
            Node::Kern { width, subtype, .. } => {
                self.u8(5);
                self.i32(*width);
                self.u8(*subtype);
            }
            Node::MarginKern {
                width,
                left,
                font,
                ch,
            } => {
                self.u8(15);
                self.i32(*width);
                self.u8(u8::from(*left));
                self.i32(i32::from(font.0));
                self.u8(*ch);
            }
            Node::Penalty(p) => {
                self.u8(6);
                self.i32(*p);
            }
            Node::Math { width, subtype, .. } => {
                self.u8(7);
                self.i32(*width);
                self.u8(*subtype);
            }
            Node::Ligature(l) => {
                self.u8(8);
                self.i32(i32::from(l.font.0));
                self.u8(l.ch);
                self.u8(l.subtype);
                self.bytes(&l.original);
            }
            Node::Disc(d) => {
                self.u8(9);
                self.list(&d.pre);
                self.list(&d.post);
                self.list(&d.replace);
            }
            Node::Ins(i) => {
                self.u8(10);
                self.u8(i.number);
                self.i32(i.height);
                self.glue(&i.split_top);
                self.i32(i.split_max_depth);
                self.i32(i.float_cost);
                self.list(&i.list);
            }
            Node::Mark(m) => {
                self.u8(11);
                self.i32(m.class);
                self.ints(&m.tokens);
            }
            Node::Adjust(a) => {
                self.u8(12);
                self.u8(u8::from(a.pre));
                self.list(&a.list);
            }
            Node::Whatsit(w) => {
                self.u8(13);
                self.whatsit(w);
            }
            Node::Unset(u) => {
                self.u8(14);
                for x in [u.width, u.height, u.depth] {
                    self.i32(x);
                }
                self.i32(i32::from(u.span_count));
                self.i32(u.stretch);
                self.i32(u.shrink);
                self.order(u.stretch_order);
                self.order(u.shrink_order);
                self.list(&u.list);
            }
        }
    }
    fn whatsit(&mut self, w: &Whatsit) {
        match w {
            Whatsit::Open {
                stream,
                name,
                area,
                ext,
            } => {
                self.u8(0);
                self.i32(*stream);
                self.bytes(name);
                self.bytes(area);
                self.bytes(ext);
            }
            Whatsit::Write { stream, tokens } => {
                self.u8(1);
                self.i32(*stream);
                self.ints(tokens);
            }
            Whatsit::Close { stream } => {
                self.u8(2);
                self.i32(*stream);
            }
            Whatsit::Special { tokens } => {
                self.u8(3);
                self.ints(tokens);
            }
            Whatsit::Language {
                language,
                left_hyphen_min,
                right_hyphen_min,
            } => {
                self.u8(4);
                self.i32(*language);
                self.u8(*left_hyphen_min);
                self.u8(*right_hyphen_min);
            }
            Whatsit::LateSpecial { tokens } => {
                self.u8(5);
                self.ints(tokens);
            }
            Whatsit::Pdf(p) => {
                self.u8(6);
                self.pdf(p);
            }
            Whatsit::NativeWord(w) => {
                self.u8(7);
                self.i32(i32::from(w.font.0));
                self.u8(u8::from(w.actual_text));
                self.count(w.text.len());
                for &u in w.text.iter() {
                    self.i32(i32::from(u));
                }
                for x in [w.width, w.height, w.depth] {
                    self.i32(x);
                }
                self.count(w.glyphs.len());
                for g in w.glyphs.iter() {
                    self.i32(i32::from(g.gid));
                    self.i32(g.x);
                    self.i32(g.y);
                    self.i32(i32::try_from(g.cluster).unwrap_or(i32::MAX));
                }
            }
            Whatsit::Glyph(g) => {
                self.u8(8);
                self.i32(i32::from(g.font.0));
                self.i32(i32::from(g.gid));
                for x in [g.width, g.height, g.depth] {
                    self.i32(x);
                }
            }
            Whatsit::Pic(p) => {
                self.u8(9);
                self.u8(u8::from(p.pdf));
                self.bytes(&p.path);
                self.i32(p.page);
                self.u8(p.pdf_box);
                for &t in &p.transform {
                    self.i32(t);
                }
                for x in [p.width, p.height, p.depth] {
                    self.i32(x);
                }
            }
        }
    }
    fn opt_tokens(&mut self, t: Option<&[i32]>) {
        match t {
            None => self.u8(0),
            Some(t) => {
                self.u8(1);
                self.ints(t);
            }
        }
    }
    fn opt_i32(&mut self, x: Option<i32>) {
        self.u8(u8::from(x.is_some()));
        self.i32(x.unwrap_or(0));
    }
    fn dims(&mut self, d: Dims) {
        self.i32(d.width);
        self.i32(d.height);
        self.i32(d.depth);
    }
    fn pdf_id(&mut self, id: &PdfId) {
        match id {
            PdfId::Num(n) => {
                self.u8(0);
                self.i32(*n);
            }
            PdfId::Name(t) => {
                self.u8(1);
                self.ints(t);
            }
        }
    }
    fn pdf(&mut self, w: &PdfWhatsit) {
        match w {
            PdfWhatsit::Literal { late, mode, data } => {
                self.u8(0);
                self.u8(u8::from(*late));
                self.u8(*mode);
                self.ints(data);
            }
            PdfWhatsit::ColorStack { stack, cmd, data } => {
                self.u8(1);
                self.i32(*stack);
                self.u8(*cmd);
                self.opt_tokens(data.as_deref().map(crate::node::TokenList::tokens));
            }
            PdfWhatsit::SetMatrix { data } => {
                self.u8(2);
                self.ints(data);
            }
            PdfWhatsit::Save => self.u8(3),
            PdfWhatsit::Restore => self.u8(4),
            PdfWhatsit::RefObj { objnum } => {
                self.u8(5);
                self.i32(*objnum);
            }
            PdfWhatsit::RefXForm { objnum, dims } => {
                self.u8(6);
                self.i32(*objnum);
                self.dims(*dims);
            }
            PdfWhatsit::RefXImage { objnum, dims } => {
                self.u8(7);
                self.i32(*objnum);
                self.dims(*dims);
            }
            PdfWhatsit::Annot { dims, data, objnum } => {
                self.u8(8);
                self.dims(*dims);
                self.ints(data);
                self.i32(*objnum);
            }
            PdfWhatsit::StartLink {
                dims,
                attr,
                action,
                objnum,
            } => {
                self.u8(9);
                self.dims(*dims);
                self.opt_tokens(attr.as_deref().map(crate::node::TokenList::tokens));
                self.u8(action.kind);
                self.opt_tokens(action.tokens.as_deref().map(crate::node::TokenList::tokens));
                self.pdf_id(&action.id);
                self.opt_tokens(action.file.as_deref().map(crate::node::TokenList::tokens));
                match &action.struct_id {
                    None => self.u8(0),
                    Some(id) => {
                        self.u8(1);
                        self.pdf_id(id);
                    }
                }
                self.u8(action.new_window);
                self.i32(*objnum);
            }
            PdfWhatsit::EndLink => self.u8(10),
            PdfWhatsit::Dest {
                dims,
                struct_num,
                id,
                kind,
                zoom,
            } => {
                self.u8(11);
                self.dims(*dims);
                self.opt_i32(*struct_num);
                self.pdf_id(id);
                self.u8(*kind);
                self.opt_i32(*zoom);
            }
            PdfWhatsit::Thread {
                start,
                dims,
                attr,
                id,
            } => {
                self.u8(12);
                self.u8(u8::from(*start));
                self.dims(*dims);
                self.opt_tokens(attr.as_deref().map(crate::node::TokenList::tokens));
                self.pdf_id(id);
            }
            PdfWhatsit::EndThread => self.u8(13),
            PdfWhatsit::SavePos => self.u8(14),
            PdfWhatsit::SnapRefPoint => self.u8(15),
            PdfWhatsit::SnapY { glue, final_skip } => {
                self.u8(16);
                self.glue(glue);
                self.i32(*final_skip);
            }
            PdfWhatsit::SnapYComp { ratio } => {
                self.u8(17);
                self.i32(*ratio);
            }
            PdfWhatsit::InterwordSpaceOn => self.u8(18),
            PdfWhatsit::InterwordSpaceOff => self.u8(19),
            PdfWhatsit::FakeSpace => self.u8(20),
            PdfWhatsit::RunningLinkOff => self.u8(21),
            PdfWhatsit::RunningLinkOn => self.u8(22),
        }
    }
}

/// A decoder over `data`.
#[derive(Clone, Debug)]
pub struct Dec<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Dec<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Dec { data, pos: 0 }
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn count(&mut self) -> Option<usize> {
        let n = usize::try_from(self.i32()?).ok()?;
        // (every item takes at least a byte: reject absurd lengths early)
        (n <= self.data.len() - self.pos).then_some(n)
    }
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.count()?;
        self.take(n)
    }
    pub fn ints(&mut self) -> Option<Vec<i32>> {
        let n = self.count()?;
        (0..n).map(|_| self.i32()).collect()
    }
    fn scaled(&mut self) -> Option<Scaled> {
        self.i32()
    }
    fn order(&mut self) -> Option<Order> {
        Order::ALL.get(usize::from(self.u8()?)).copied()
    }
    fn font(&mut self) -> Option<FontId> {
        Some(FontId(u16::try_from(self.i32()?).ok()?))
    }
    pub fn glue(&mut self) -> Option<GlueSpec> {
        Some(GlueSpec {
            width: self.scaled()?,
            stretch: self.scaled()?,
            shrink: self.scaled()?,
            stretch_order: self.order()?,
            shrink_order: self.order()?,
            shared_zero: self.u8()? != 0,
        })
    }
    pub fn box_node(&mut self) -> Option<BoxNode> {
        let vertical = self.u8()? != 0;
        let width = self.scaled()?;
        let height = self.scaled()?;
        let depth = self.scaled()?;
        let shift = self.scaled()?;
        let glue_set = f64::from_bits(u64::from_le_bytes(self.take(8)?.try_into().ok()?));
        let sign = self.u8()?;
        let glue_sign = match sign & 0x7f {
            0 => GlueSign::Normal,
            1 => GlueSign::Stretching,
            2 => GlueSign::Shrinking,
            _ => return None,
        };
        let sync = if sign & 0x80 == 0 {
            crate::origin::Side(0)
        } else {
            crate::origin::Side(self.i32()?.cast_unsigned())
        };
        let glue_order = self.order()?;
        let subtype = self.u8()?;
        let seal = if subtype & 0x80 == 0 {
            None
        } else {
            Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
        };
        Some(BoxNode {
            vertical,
            width,
            height,
            depth,
            shift,
            glue_set,
            glue_sign,
            glue_order,
            subtype: subtype & 0x7f,
            list: self.list()?,
            seal,
            ver: 0,
            sync,
        })
    }
    pub fn list(&mut self) -> Option<Vec<Node>> {
        let n = self.count()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            // (a run is put back by its characters: a run longer than
            // `Glyphs::CAP`, which a format made when runs were longer
            // holds, becomes canonical runs)
            if self.data.get(self.pos) == Some(&0) {
                self.pos += 1;
                let font = self.font()?;
                let chars = self.bytes()?;
                if chars.is_empty() {
                    return None;
                }
                for &c in chars {
                    crate::node::push_char(&mut out, font, c);
                }
                continue;
            }
            out.push(self.node()?);
        }
        Some(out)
    }
    fn tokens(&mut self) -> Option<crate::node::Tokens> {
        Some(Arc::new(crate::node::TokenList::new(self.ints()?, false)))
    }
    pub fn node(&mut self) -> Option<Node> {
        if self.data.get(self.pos) == Some(&16) {
            self.pos += 1;
            let sync = crate::origin::Side(self.i32()?.cast_unsigned());
            let mut n = self.node()?;
            set_place(&mut n, sync)?;
            return Some(n);
        }
        Some(match self.u8()? {
            0 => {
                let font = self.font()?;
                let chars = self.bytes()?;
                let (&first, rest) = chars.split_first()?;
                if chars.len() > Glyphs::CAP {
                    return None;
                }
                let mut g = Glyphs::one(font, first);
                for &c in rest {
                    g.push(c);
                }
                Node::Glyphs(g)
            }
            1 => Node::Box((self.box_node()?).share()),
            2 => Node::Rule {
                width: self.scaled()?,
                height: self.scaled()?,
                depth: self.scaled()?,
                sync: crate::origin::Side(0),
            },
            3 => Node::Glue {
                spec: self.glue()?,
                subtype: self.u8()?,
                sync: crate::origin::Side(0),
            },
            4 => {
                let spec = self.glue()?;
                let kind = match self.u8()? {
                    0 => Leaders::Aligned,
                    1 => Leaders::Centered,
                    2 => Leaders::Expanded,
                    _ => return None,
                };
                let leader = self.node()?;
                Node::Leaders(Box::new(LeaderNode {
                    spec,
                    kind,
                    leader,
                    sync: crate::origin::Side(0),
                }))
            }
            5 => Node::Kern {
                width: self.scaled()?,
                subtype: self.u8()?,
                sync: crate::origin::Side(0),
            },
            15 => Node::MarginKern {
                width: self.scaled()?,
                left: self.u8()? != 0,
                font: FontId(u16::try_from(self.i32()?).ok()?),
                ch: self.u8()?,
            },
            6 => Node::Penalty(self.i32()?),
            7 => Node::Math {
                width: self.scaled()?,
                subtype: self.u8()?,
                sync: crate::origin::Side(0),
            },
            8 => Node::Ligature(Box::new(Ligature {
                font: self.font()?,
                ch: self.u8()?,
                subtype: self.u8()?,
                original: self.bytes()?.to_vec(),
                org: crate::origin::Side::default(),
            })),
            9 => Node::Disc(Box::new(Disc {
                pre: self.list()?,
                post: self.list()?,
                replace: self.list()?,
            })),
            10 => Node::Ins(Box::new(Ins {
                number: self.u8()?,
                height: self.scaled()?,
                split_top: self.glue()?,
                split_max_depth: self.scaled()?,
                float_cost: self.i32()?,
                list: self.list()?,
            })),
            11 => Node::Mark(Box::new(Mark {
                class: self.i32()?,
                tokens: self.tokens()?,
            })),
            12 => Node::Adjust(Box::new(Adjust {
                pre: self.u8()? != 0,
                list: self.list()?,
            })),
            13 => Node::Whatsit(Box::new(self.whatsit()?)),
            14 => Node::Unset(Box::new(Unset {
                width: self.scaled()?,
                height: self.scaled()?,
                depth: self.scaled()?,
                span_count: u16::try_from(self.i32()?).ok()?,
                stretch: self.scaled()?,
                shrink: self.scaled()?,
                stretch_order: self.order()?,
                shrink_order: self.order()?,
                list: self.list()?,
                sync: crate::origin::Side(0),
            })),
            _ => return None,
        })
    }
    fn whatsit(&mut self) -> Option<Whatsit> {
        Some(match self.u8()? {
            0 => Whatsit::Open {
                stream: self.i32()?,
                name: Arc::from(self.bytes()?),
                area: Arc::from(self.bytes()?),
                ext: Arc::from(self.bytes()?),
            },
            1 => Whatsit::Write {
                stream: self.i32()?,
                tokens: self.tokens()?,
            },
            2 => Whatsit::Close {
                stream: self.i32()?,
            },
            3 => Whatsit::Special {
                tokens: self.tokens()?,
            },
            4 => Whatsit::Language {
                language: self.i32()?,
                left_hyphen_min: self.u8()?,
                right_hyphen_min: self.u8()?,
            },
            5 => Whatsit::LateSpecial {
                tokens: self.tokens()?,
            },
            6 => Whatsit::Pdf(Box::new(self.pdf()?)),
            7 => {
                let font = self.font()?;
                let actual_text = self.u8()? != 0;
                let n = self.count()?;
                let mut text = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    text.push(u16::try_from(self.i32()?).ok()?);
                }
                let (width, height, depth) = (self.i32()?, self.i32()?, self.i32()?);
                let n = self.count()?;
                let mut glyphs = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    glyphs.push(crate::native::NativeGlyph {
                        gid: u16::try_from(self.i32()?).ok()?,
                        x: self.i32()?,
                        y: self.i32()?,
                        cluster: u32::try_from(self.i32()?).ok()?,
                    });
                }
                Whatsit::NativeWord(crate::native::NativeWord {
                    font,
                    actual_text,
                    text: Arc::from(text),
                    width,
                    height,
                    depth,
                    glyphs: Arc::from(glyphs),
                    org: crate::origin::Side(0),
                })
            }
            8 => Whatsit::Glyph(crate::native::GlyphNode {
                font: self.font()?,
                gid: u16::try_from(self.i32()?).ok()?,
                width: self.i32()?,
                height: self.i32()?,
                depth: self.i32()?,
            }),
            9 => {
                let pdf = self.u8()? != 0;
                let path = Arc::from(self.bytes()?);
                let page = self.i32()?;
                let pdf_box = self.u8()?;
                let mut transform = [0; 6];
                for t in &mut transform {
                    *t = self.i32()?;
                }
                Whatsit::Pic(Box::new(crate::native::PicNode {
                    pdf,
                    path,
                    page,
                    pdf_box,
                    transform,
                    width: self.i32()?,
                    height: self.i32()?,
                    depth: self.i32()?,
                }))
            }
            _ => return None,
        })
    }
    #[expect(clippy::option_option, reason = "outer: malformed input")]
    fn opt_tokens(&mut self) -> Option<Option<crate::node::Tokens>> {
        Some(match self.u8()? {
            0 => None,
            _ => Some(self.tokens()?),
        })
    }
    #[expect(clippy::option_option, reason = "outer: malformed input")]
    fn opt_i32(&mut self) -> Option<Option<i32>> {
        let some = self.u8()? != 0;
        let x = self.i32()?;
        Some(some.then_some(x))
    }
    fn dims(&mut self) -> Option<Dims> {
        Some(Dims {
            width: self.i32()?,
            height: self.i32()?,
            depth: self.i32()?,
        })
    }
    fn pdf_id(&mut self) -> Option<PdfId> {
        Some(match self.u8()? {
            0 => PdfId::Num(self.i32()?),
            _ => PdfId::Name(self.tokens()?),
        })
    }
    fn pdf(&mut self) -> Option<PdfWhatsit> {
        Some(match self.u8()? {
            0 => PdfWhatsit::Literal {
                late: self.u8()? != 0,
                mode: self.u8()?,
                data: self.tokens()?,
            },
            1 => PdfWhatsit::ColorStack {
                stack: self.i32()?,
                cmd: self.u8()?,
                data: self.opt_tokens()?,
            },
            2 => PdfWhatsit::SetMatrix {
                data: self.tokens()?,
            },
            3 => PdfWhatsit::Save,
            4 => PdfWhatsit::Restore,
            5 => PdfWhatsit::RefObj {
                objnum: self.i32()?,
            },
            6 => PdfWhatsit::RefXForm {
                objnum: self.i32()?,
                dims: self.dims()?,
            },
            7 => PdfWhatsit::RefXImage {
                objnum: self.i32()?,
                dims: self.dims()?,
            },
            8 => PdfWhatsit::Annot {
                dims: self.dims()?,
                data: self.tokens()?,
                objnum: self.i32()?,
            },
            9 => {
                let dims = self.dims()?;
                let attr = self.opt_tokens()?;
                let kind = self.u8()?;
                let tokens = self.opt_tokens()?;
                let id = self.pdf_id()?;
                let file = self.opt_tokens()?;
                let struct_id = match self.u8()? {
                    0 => None,
                    _ => Some(self.pdf_id()?),
                };
                let new_window = self.u8()?;
                PdfWhatsit::StartLink {
                    dims,
                    attr,
                    action: Arc::new(Action {
                        kind,
                        tokens,
                        id,
                        file,
                        struct_id,
                        new_window,
                    }),
                    objnum: self.i32()?,
                }
            }
            10 => PdfWhatsit::EndLink,
            11 => PdfWhatsit::Dest {
                dims: self.dims()?,
                struct_num: self.opt_i32()?,
                id: self.pdf_id()?,
                kind: self.u8()?,
                zoom: self.opt_i32()?,
            },
            12 => PdfWhatsit::Thread {
                start: self.u8()? != 0,
                dims: self.dims()?,
                attr: self.opt_tokens()?,
                id: self.pdf_id()?,
            },
            13 => PdfWhatsit::EndThread,
            14 => PdfWhatsit::SavePos,
            15 => PdfWhatsit::SnapRefPoint,
            16 => PdfWhatsit::SnapY {
                glue: self.glue()?,
                final_skip: self.i32()?,
            },
            17 => PdfWhatsit::SnapYComp { ratio: self.i32()? },
            18 => PdfWhatsit::InterwordSpaceOn,
            19 => PdfWhatsit::InterwordSpaceOff,
            20 => PdfWhatsit::FakeSpace,
            21 => PdfWhatsit::RunningLinkOff,
            22 => PdfWhatsit::RunningLinkOn,
            _ => return None,
        })
    }
}

/// The inline `SyncTeX` place of a node that has one outside a box's
/// encoding (`origin::Side::INLINE`).
fn inline_place(n: &Node) -> Option<crate::origin::Side> {
    let s = match n {
        Node::Rule { sync, .. }
        | Node::Glue { sync, .. }
        | Node::Kern { sync, .. }
        | Node::Math { sync, .. } => *sync,
        Node::Leaders(l) => l.sync,
        Node::Unset(u) => u.sync,
        _ => return None,
    };
    s.is_inline().then_some(s)
}

/// Give a decoded node the inline place encoded before it (`None`: a node
/// that has none).
fn set_place(n: &mut Node, s: crate::origin::Side) -> Option<()> {
    match n {
        Node::Rule { sync, .. }
        | Node::Glue { sync, .. }
        | Node::Kern { sync, .. }
        | Node::Math { sync, .. } => *sync = s,
        Node::Leaders(l) => l.sync = s,
        Node::Unset(u) => u.sync = s,
        _ => return None,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Every kind of node survives a round trip.
    #[test]
    fn round_trip() {
        let glue = GlueSpec {
            width: 3,
            stretch: 4,
            shrink: 5,
            stretch_order: Order::Fil,
            shrink_order: Order::Filll,
            shared_zero: false,
        };
        let inner = BoxNode {
            vertical: true,
            width: 1,
            glue_set: 0.25,
            glue_sign: GlueSign::Shrinking,
            glue_order: Order::Fill,
            list: vec![Node::Penalty(-7)],
            ..BoxNode::default()
        };
        let mut g = Glyphs::one(FontId(3), b'a');
        g.push(b'b');
        let list = vec![
            Node::Glyphs(g),
            Node::Box((inner.clone()).share()),
            Node::Rule {
                width: 1,
                height: 2,
                depth: 3,
                sync: crate::origin::Side(0),
            },
            Node::Glue {
                spec: glue,
                subtype: 99,
                sync: crate::origin::Side(0),
            },
            Node::Leaders(Box::new(LeaderNode {
                spec: glue,
                kind: Leaders::Centered,
                leader: Node::Box(inner.share()),
                sync: crate::origin::Side(0),
            })),
            Node::Kern {
                width: -2,
                subtype: 1,
                sync: crate::origin::Side(0),
            },
            Node::Math {
                width: 0,
                subtype: 1,
                sync: crate::origin::Side(0),
            },
            Node::Ligature(Box::new(Ligature {
                font: FontId(1),
                ch: 11,
                subtype: 2,
                original: vec![b'f', b'f'],
                org: crate::origin::Side::default(),
            })),
            Node::Disc(Box::new(Disc {
                pre: vec![Node::Penalty(1)],
                post: vec![],
                replace: vec![Node::Penalty(2)],
            })),
            Node::Ins(Box::new(Ins {
                number: 100,
                height: 9,
                split_top: glue,
                split_max_depth: 8,
                float_cost: 7,
                list: vec![Node::Penalty(3)],
            })),
            Node::Mark(Box::new(Mark {
                class: 0,
                tokens: crate::node::TokenList::shared(&[1, 2, 3]),
            })),
            Node::Adjust(Box::new(Adjust {
                pre: true,
                list: vec![],
            })),
            Node::Whatsit(Box::new(Whatsit::Open {
                stream: 3,
                name: Arc::from(&b"x"[..]),
                area: Arc::from(&b""[..]),
                ext: Arc::from(&b".tex"[..]),
            })),
            Node::Whatsit(Box::new(Whatsit::Language {
                language: 2,
                left_hyphen_min: 1,
                right_hyphen_min: 3,
            })),
            Node::Unset(Box::new(Unset {
                span_count: 2,
                stretch_order: Order::Fil,
                ..Unset::default()
            })),
        ];
        let mut e = Enc::default();
        e.list(&list);
        let mut d = Dec::new(&e.0);
        assert_eq!(d.list().unwrap(), list);
        assert_eq!(d.pos, e.0.len());
        // Truncated input is rejected, not a panic.
        for n in 0..e.0.len() {
            assert!(Dec::new(&e.0[..n]).list().is_none());
        }
    }
}
