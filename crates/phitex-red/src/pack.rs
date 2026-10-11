//! C2: `hpack`, `vpack` as pure functions (tex.web 649–679).
use std::sync::Arc;
use crate::item::{BoxNode, Item};
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spec { Exactly(i32), Additional(i32) }
/// What packing reads from env, read by the caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params { pub hbadness: i32, pub vbadness: i32, pub hfuzz: i32, pub vfuzz: i32, pub overfull_rule: i32 }
#[derive(Clone, Debug)]
pub struct Packed { pub b: BoxNode, pub badness: i32, pub report: Option<Arc<crate::diag::Diag>>, pub adjust: Vec<Item> }
pub fn hpack(_list: Vec<Item>, _spec: Spec, _p: &Params, _fonts: &dyn crate::font::Fonts) -> Packed { todo!("C2") }
pub fn vpack(_list: Vec<Item>, _spec: Spec, _max_depth: i32, _p: &Params) -> Packed { todo!("C2") }
