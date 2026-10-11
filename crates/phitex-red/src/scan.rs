//! B3: scanning (tex.web part 26–27): numbers, dimensions, glue, keywords,
//! token lists, internal quantities via the registry, e-TeX expressions.
use crate::ctx::Ctx;
use crate::prim::{Level, Registry};
use crate::tok::{Tok, Toks};
use crate::val::{Glue, Val};
pub fn register(_r: &mut Registry) {}
pub fn scan_int(_c: &mut dyn Ctx) -> i32 { todo!("B3") }
pub fn scan_dimen(_c: &mut dyn Ctx, _mu: bool, _inf: bool) -> (i32, u8) { todo!("B3") }
pub fn scan_normal_dimen(_c: &mut dyn Ctx) -> i32 { todo!("B3") }
pub fn scan_glue(_c: &mut dyn Ctx, _level: Level) -> Glue { todo!("B3") }
pub fn scan_keyword(_c: &mut dyn Ctx, _kw: &str) -> bool { todo!("B3") }
pub fn scan_optional_equals(_c: &mut dyn Ctx) { todo!("B3") }
pub fn scan_left_brace(_c: &mut dyn Ctx) { todo!("B3") }
/// `macro_def`: a parameter text then body; `xpand`: expand as `\edef`.
pub fn scan_toks(_c: &mut dyn Ctx, _macro_def: bool, _xpand: bool) -> (Toks, Toks) { todo!("B3") }
/// `\edef`/`\write`/`\message` expansion of a balanced text already read.
pub fn expand_toks(_c: &mut dyn Ctx, _t: &Toks) -> Toks { todo!("B3") }
pub fn scan_char_num(_c: &mut dyn Ctx) -> u16 { todo!("B3") }
pub fn scan_register_num(_c: &mut dyn Ctx) -> u16 { todo!("B3") }
pub fn scan_eight_bit_int(_c: &mut dyn Ctx) -> u8 { todo!("B3") }
pub fn scan_four_bit_int(_c: &mut dyn Ctx) -> u8 { todo!("B3") }
pub fn scan_fifteen_bit_int(_c: &mut dyn Ctx) -> u32 { todo!("B3") }
pub fn scan_twenty_seven_bit_int(_c: &mut dyn Ctx) -> u32 { todo!("B3") }
pub fn scan_font_ident(_c: &mut dyn Ctx) -> crate::val::FontId { todo!("B3") }
pub fn scan_file_name(_c: &mut dyn Ctx) -> String { todo!("B3") }
pub fn scan_general_text(_c: &mut dyn Ctx) -> Toks { todo!("B3") }
/// `scan_something_internal`: the value of the quantity token `t` at `level`.
pub fn scan_internal(_c: &mut dyn Ctx, _t: Tok, _level: Level, _neg: bool) -> (Val, Level) { todo!("B3") }
/// Non-blank non-`\relax` token (`get_x_non_blank`-style helpers).
pub fn next_non_blank(_c: &mut dyn Ctx) -> Option<Tok> { todo!("B3") }
