//! Material: immutable items nodes emit.
use std::sync::Arc;

use crate::tok::Toks;
use crate::val::{FontId, Glue};

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct GlueSet {
    /// 0 normal, 1 stretching, -1 shrinking.
    pub sign: i8,
    pub order: u8,
    pub ratio: f64,
}
impl Eq for GlueSet {}

pub const RUNNING: i32 = -0o10_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BoxNode {
    /// false hlist, true vlist.
    pub vertical: bool,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub shift: i32,
    pub set: GlueSet,
    /// e-TeX TeXXeT: 0 normal, 1 reversed, 2 dlist.
    pub lr: u8,
    pub list: Arc<[Item]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disc {
    pub pre: Arc<[Item]>,
    pub post: Arc<[Item]>,
    pub replace: u16,
    pub explicit: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ins {
    pub class: u8,
    pub height: i32,
    pub depth: i32,
    pub split_top: Glue,
    pub split_max_depth: i32,
    pub float_cost: i32,
    pub list: Arc<[Item]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Whatsit {
    Open { stream: u8, name: Arc<str>, area: Arc<str>, ext: Arc<str> },
    /// Tokens expanded at shipout.
    Write { stream: u8, toks: Toks },
    Close { stream: u8 },
    Special { toks: Toks },
    Language { lang: u8, lhm: u8, rhm: u8 },
    /// pdfTeX whatsits, owned by `pdfprim.rs`.
    Pdf(Arc<crate::pdfprim::PdfWhat>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Char { font: FontId, c: u16 },
    Lig { font: FontId, c: u16, orig: Arc<[u16]>, left: bool, right: bool },
    Box(Arc<BoxNode>),
    Rule { w: i32, h: i32, d: i32 },
    /// subtype: 0 normal, 1+param, 98 cond math, 99 mu; leaders: 100..=102 with `leader`.
    Glue { g: Glue, sub: u8, leader: Option<Arc<Item>> },
    /// subtype 0 font 1 explicit 2 accent 3 mu.
    Kern { w: i32, sub: u8 },
    MarginKern { w: i32, right: bool, font: FontId, c: u16 },
    Penalty(i32),
    Disc(Arc<Disc>),
    Mark { class: u16, toks: Toks },
    Ins(Arc<Ins>),
    Adjust(Arc<[Item]>),
    Whatsit(Arc<Whatsit>),
    /// Math node: width, subtype (0 before 1 after; TeXXeT LR codes).
    Math { w: i32, sub: u8 },
    /// Unset box inside an alignment (C6 owns the meaning of the fields).
    Unset(Arc<crate::align::Unset>),
    /// A math noad (only inside math lists; C5).
    Noad(Arc<crate::math::Noad>),
}
