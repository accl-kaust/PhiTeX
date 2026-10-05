//! cs_type2.c, cs_type2.h: Type 2 charstrings copied with their subrs
//! expanded.
//!
//! The C statics are [`State`] (`self.cs_type2`); the static functions
//! are its methods. Charstring input `(card8 **data, card8 *endptr)` is
//! `(data: &[u8], p: &mut usize)`; output `(card8 **dest, card8 *limit)`
//! is `(dst: &mut [u8], dp: &mut usize)` with `limit` = `dst.len()`.

use crate::cff::*;
use crate::prelude::*;

pub const CS_TYPE2_DEBUG_STR: &str = "Type2 Charstring Parser";
pub const CS_TYPE2_DEBUG: i32 = 5;

pub const CS_BUFFER_ERROR: i32 = -3;
pub const CS_STACK_ERROR: i32 = -2;
pub const CS_PARSE_ERROR: i32 = -1;
pub const CS_PARSE_OK: i32 = 0;
pub const CS_PARSE_END: i32 = 1;
pub const CS_SUBR_RETURN: i32 = 2;
pub const CS_CHAR_END: i32 = 3;

// Operators (`cs_*`; the second set follows `cs_escape`).
const CS_HSTEM: u8 = 1;
const CS_VSTEM: u8 = 3;
const CS_VMOVETO: u8 = 4;
const CS_RLINETO: u8 = 5;
const CS_HLINETO: u8 = 6;
const CS_VLINETO: u8 = 7;
const CS_RRCURVETO: u8 = 8;
const CS_CALLSUBR: u8 = 10;
const CS_RETURN: u8 = 11;
const CS_ESCAPE: u8 = 12;
const CS_ENDCHAR: u8 = 14;
const CS_HSTEMHM: u8 = 18;
const CS_HINTMASK: u8 = 19;
const CS_CNTRMASK: u8 = 20;
const CS_RMOVETO: u8 = 21;
const CS_HMOVETO: u8 = 22;
const CS_VSTEMHM: u8 = 23;
const CS_RCURVELINE: u8 = 24;
const CS_RLINECURVE: u8 = 25;
const CS_VVCURVETO: u8 = 26;
const CS_HHCURVETO: u8 = 27;
const CS_CALLGSUBR: u8 = 29;
const CS_VHCURVETO: u8 = 30;
const CS_HVCURVETO: u8 = 31;
const CS_DOTSECTION: u8 = 0;
const CS_AND: u8 = 3;
const CS_OR: u8 = 4;
const CS_NOT: u8 = 5;
const CS_ABS: u8 = 9;
const CS_ADD: u8 = 10;
const CS_SUB: u8 = 11;
const CS_DIV: u8 = 12;
const CS_NEG: u8 = 14;
const CS_EQ: u8 = 15;
const CS_DROP: u8 = 18;
const CS_PUT: u8 = 20;
const CS_GET: u8 = 21;
const CS_IFELSE: u8 = 22;
const CS_RANDOM: u8 = 23;
const CS_MUL: u8 = 24;
const CS_SQRT: u8 = 26;
const CS_DUP: u8 = 27;
const CS_EXCH: u8 = 28;
const CS_INDEX: u8 = 29;
const CS_ROLL: u8 = 30;
const CS_HFLEX: u8 = 34;
const CS_FLEX: u8 = 35;
const CS_HFLEX1: u8 = 36;
const CS_FLEX1: u8 = 37;

/// cs_type2.c's statics.
#[derive(Clone, Debug)]
pub struct State {
    pub status: i32,
    pub num_stems: i32,
    pub phase: i32,
    pub nest: i32,
    pub have_width: i32,
    pub width: f64,
    pub seac: [f64; 4],
    pub stack_top: i32,
    pub arg_stack: [f64; CS_ARG_STACK_MAX],
    pub trn_array: [f64; CS_TRANS_ARRAY_MAX],
}

impl Default for State {
    fn default() -> Self {
        State {
            status: CS_PARSE_ERROR,
            num_stems: 0,
            phase: 0,
            nest: 0,
            have_width: 0,
            width: 0.0,
            seac: [0.0; 4],
            stack_top: 0,
            arg_stack: [0.0; CS_ARG_STACK_MAX],
            trn_array: [0.0; CS_TRANS_ARRAY_MAX],
        }
    }
}

/// `bbox` of `cs_ginfo` / `t1_ginfo`.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlyphBbox {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

/// `seac` of `cs_ginfo` / `t1_ginfo`.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlyphSeac {
    pub asb: f64,
    pub adx: f64,
    pub ady: f64,
    pub bchar: Card8,
    pub achar: Card8,
}

/// `cs_ginfo`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CsGinfo {
    /// Unused in Type 2 charstrings.
    pub flags: i32,
    pub wx: f64,
    pub wy: f64,
    pub bbox: GlyphBbox,
    /// Unused in Type 2 charstrings.
    pub seac: GlyphSeac,
}

/// `get_subr` (static): subr `id` (bias applied) of `subr_idx`.
fn get_subr(subr_idx: Option<&CffIndex>, id: i32) -> &[u8] {
    todo!()
}

impl State {
    /// `clear_stack` (static).
    fn clear_stack(&mut self, dst: &mut [u8], dp: &mut usize) {
        todo!()
    }
    /// `do_operator1` (static).
    fn do_operator1(&mut self, dst: &mut [u8], dp: &mut usize, data: &[u8], p: &mut usize) {
        todo!()
    }
    /// `do_operator2` (static).
    fn do_operator2(&mut self, dst: &mut [u8], dp: &mut usize, data: &[u8], p: &mut usize) {
        todo!()
    }
    /// `get_integer` (static).
    fn get_integer(&mut self, data: &[u8], p: &mut usize) {
        todo!()
    }
    /// `get_fixed` (static).
    fn get_fixed(&mut self, data: &[u8], p: &mut usize) {
        todo!()
    }
    /// `do_charstring` (static).
    fn do_charstring(
        &mut self,
        dst: &mut [u8],
        dp: &mut usize,
        data: &[u8],
        p: &mut usize,
        gsubr_idx: Option<&CffIndex>,
        subr_idx: Option<&CffIndex>,
    ) {
        todo!()
    }
    /// `cs_parse_init` (static).
    fn cs_parse_init(&mut self) {
        todo!()
    }
}

impl Dpx {
    /// `cs_copy_charstring`: `src` copied into `dst` (`dstlen` =
    /// `dst.len()`) with subrs expanded; the bytes written.
    pub fn cs_copy_charstring(
        &mut self,
        dst: &mut [u8],
        src: &[u8],
        gsubr: Option<&CffIndex>,
        subr: Option<&CffIndex>,
        default_width: f64,
        nominal_width: f64,
        ginfo: Option<&mut CsGinfo>,
    ) -> i32 {
        todo!()
    }
}
