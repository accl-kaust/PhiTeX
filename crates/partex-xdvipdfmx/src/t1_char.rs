//! t1_char.c, t1_char.h: Type 1 charstrings decoded and re-encoded as
//! Type 2.
//!
//! The C statics are [`State`] (`self.t1_char`); the static functions
//! are its methods. Input `(card8 **data, card8 *endptr)` is
//! `(data: &[u8], p: &mut usize)`; output `(card8 **dest, card8 *limit)`
//! is `(dst: &mut [u8], dp: &mut usize)`. The `t1_cpath` linked list
//! (`charpath` .. `lastpath`) is the `Vec` [`T1Chardesc::charpath`].

use crate::cff::{CS_ARG_STACK_MAX, CS_STEM_ZONE_MAX, Card8, CffIndex};
use crate::cs_type2::{GlyphBbox, GlyphSeac};
use crate::prelude::*;

pub const CS_OP_NOSUPPORT: i32 = -4;
pub const CS_BUFFER_ERROR: i32 = -3;
pub const CS_STACK_ERROR: i32 = -2;
pub const CS_PARSE_ERROR: i32 = -1;
pub const CS_PARSE_OK: i32 = 0;
pub const CS_PARSE_END: i32 = 1;
pub const CS_SUBR_RETURN: i32 = 2;
pub const CS_CHAR_END: i32 = 3;

pub const T1_CS_PHASE_INIT: i32 = 0;
pub const T1_CS_PHASE_HINT: i32 = 1;
pub const T1_CS_PHASE_PATH: i32 = 2;
pub const T1_CS_PHASE_FLEX: i32 = 3;

pub const CS_STEM_GROUP_MAX: usize = CS_STEM_ZONE_MAX;
/// Counter control may have `CS_STEM_ZONE_MAX * 2 + 2` arguments.
pub const PS_ARG_STACK_MAX: usize = CS_STEM_ZONE_MAX * 2 + 2;

pub const HSTEM: i32 = 0;
pub const VSTEM: i32 = 1;

pub const T1_CS_FLAG_NONE: i32 = 0;
pub const T1_CS_FLAG_USE_HINTMASK: i32 = 1 << 0;
pub const T1_CS_FLAG_USE_CNTRMASK: i32 = 1 << 1;
pub const T1_CS_FLAG_USE_SEAC: i32 = 1 << 2;

pub const CS_HINT_DECL: i32 = -1;
pub const CS_FLEX_CTRL: i32 = -2;
pub const CS_CNTR_CTRL: i32 = -3;

// Operators (`cs_*`; the second set follows `cs_escape`).
const CS_HSTEM: u8 = 1;
const CS_VSTEM: u8 = 3;
const CS_VMOVETO: u8 = 4;
const CS_RLINETO: u8 = 5;
const CS_HLINETO: u8 = 6;
const CS_VLINETO: u8 = 7;
const CS_RRCURVETO: u8 = 8;
const CS_CLOSEPATH: u8 = 9;
const CS_CALLSUBR: u8 = 10;
const CS_RETURN: u8 = 11;
const CS_ESCAPE: u8 = 12;
const CS_HSBW: u8 = 13;
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
const CS_VSTEM3: u8 = 1;
const CS_HSTEM3: u8 = 2;
const CS_AND: u8 = 3;
const CS_OR: u8 = 4;
const CS_NOT: u8 = 5;
const CS_SEAC: u8 = 6;
const CS_SBW: u8 = 7;
const CS_ABS: u8 = 9;
const CS_ADD: u8 = 10;
const CS_SUB: u8 = 11;
const CS_DIV: u8 = 12;
const CS_NEG: u8 = 14;
const CS_EQ: u8 = 15;
const CS_CALLOTHERSUBR: u8 = 16;
const CS_POP: u8 = 17;
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
const CS_SETCURRENTPOINT: u8 = 33;
const CS_HFLEX: u8 = 34;
const CS_FLEX: u8 = 35;
const CS_HFLEX1: u8 = 36;
const CS_FLEX1: u8 = 37;

/// `IS_PATH_OPERATOR`.
#[must_use]
pub fn is_path_operator(o: i32) -> bool {
    todo!()
}

/// t1_char.c's statics (`[vh]stem` support needs one more slot on
/// `cs_arg_stack`).
#[derive(Clone, Debug)]
pub struct State {
    pub status: i32,
    pub phase: i32,
    pub nest: i32,
    pub cs_stack_top: i32,
    pub ps_stack_top: i32,
    pub cs_arg_stack: [f64; CS_ARG_STACK_MAX + 1],
    pub ps_arg_stack: [f64; PS_ARG_STACK_MAX],
}

impl Default for State {
    fn default() -> Self {
        State {
            status: CS_PARSE_ERROR,
            phase: -1,
            nest: -1,
            cs_stack_top: 0,
            ps_stack_top: 0,
            cs_arg_stack: [0.0; CS_ARG_STACK_MAX + 1],
            ps_arg_stack: [0.0; PS_ARG_STACK_MAX],
        }
    }
}

/// `t1_ginfo`.
#[derive(Clone, Copy, Debug, Default)]
pub struct T1Ginfo {
    pub use_seac: i32,
    pub wx: f64,
    pub wy: f64,
    pub bbox: GlyphBbox,
    pub seac: GlyphSeac,
}

/// `t1_cpath` (without `next`: a [`T1Chardesc::charpath`] element).
#[derive(Clone, Debug)]
pub struct T1Cpath {
    pub type_: i32,
    pub num_args: i32,
    pub args: [f64; CS_ARG_STACK_MAX],
}

impl Default for T1Cpath {
    fn default() -> Self {
        T1Cpath {
            type_: 0,
            num_args: 0,
            args: [0.0; CS_ARG_STACK_MAX],
        }
    }
}

/// `t1_stem`.
#[derive(Clone, Copy, Debug, Default)]
pub struct T1Stem {
    pub id: i32,
    pub dir: i32,
    pub pos: f64,
    pub del: f64,
}

/// `t1_stemgroup`.
#[derive(Clone, Debug)]
pub struct T1Stemgroup {
    pub num_stems: i32,
    pub stems: [f64; CS_STEM_ZONE_MAX],
}

impl Default for T1Stemgroup {
    fn default() -> Self {
        T1Stemgroup {
            num_stems: 0,
            stems: [0.0; CS_STEM_ZONE_MAX],
        }
    }
}

/// `sbw` of `t1_chardesc`.
#[derive(Clone, Copy, Debug, Default)]
pub struct T1Sbw {
    pub sbx: f64,
    pub sby: f64,
    pub wx: f64,
    pub wy: f64,
}

/// `t1_chardesc`.
#[derive(Clone, Debug)]
pub struct T1Chardesc {
    pub flags: i32,
    pub sbw: T1Sbw,
    pub bbox: GlyphBbox,
    pub seac: GlyphSeac,
    pub num_stems: i32,
    pub stems: [T1Stem; CS_STEM_ZONE_MAX],
    /// `charpath` .. `lastpath` (the last element).
    pub charpath: Vec<T1Cpath>,
}

impl Default for T1Chardesc {
    fn default() -> Self {
        T1Chardesc {
            flags: 0,
            sbw: T1Sbw::default(),
            bbox: GlyphBbox::default(),
            seac: GlyphSeac::default(),
            num_stems: 0,
            stems: [T1Stem::default(); CS_STEM_ZONE_MAX],
            charpath: Vec::new(),
        }
    }
}

/// `stem_compare` (static): qsort order of two stems.
fn stem_compare(s1: &T1Stem, s2: &T1Stem) -> i32 {
    todo!()
}

/// `get_stem` (static): the index of stem `stem_id`, or `num_stems`.
fn get_stem(cd: &T1Chardesc, stem_id: i32) -> i32 {
    todo!()
}

/// `add_stem` (static): the stem's id, or -1.
fn add_stem(cd: &mut T1Chardesc, pos: f64, del: f64, dir: i32) -> i32 {
    todo!()
}

/// `copy_args` (static).
fn copy_args(args1: &mut [f64], args2: &[f64], count: i32) {
    todo!()
}

/// `add_charpath` (static).
fn add_charpath(cd: &mut T1Chardesc, type_: i32, argv: &[f64], argn: i32) {
    todo!()
}

/// `init_charpath` (static).
fn init_charpath(cd: &mut T1Chardesc) {
    todo!()
}

/// `release_charpath` (static).
fn release_charpath(cd: &mut T1Chardesc) {
    todo!()
}

impl State {
    /// `RESET_STATE()`.
    fn reset_state(&mut self) {
        todo!()
    }
    /// `do_operator1` (static).
    fn do_operator1(&mut self, cd: &mut T1Chardesc, data: &[u8], p: &mut usize) {
        todo!()
    }
    /// `do_othersubr0` (static).
    fn do_othersubr0(&mut self, cd: &mut T1Chardesc) {
        todo!()
    }
    /// `do_othersubr1` (static).
    fn do_othersubr1(&mut self) {
        todo!()
    }
    /// `do_othersubr2` (static).
    fn do_othersubr2(&mut self, cd: &mut T1Chardesc) {
        todo!()
    }
    /// `do_othersubr3` (static).
    fn do_othersubr3(&mut self, cd: &mut T1Chardesc) {
        todo!()
    }
    /// `do_othersubr12` (static).
    fn do_othersubr12(&mut self) {
        todo!()
    }
    /// `do_othersubr13` (static).
    fn do_othersubr13(&mut self, cd: &mut T1Chardesc) {
        todo!()
    }
    /// `do_callothersubr` (static).
    fn do_callothersubr(&mut self, cd: &mut T1Chardesc) {
        todo!()
    }
    /// `do_operator2` (static).
    fn do_operator2(&mut self, cd: &mut T1Chardesc, data: &[u8], p: &mut usize) {
        todo!()
    }
    /// `put_numbers` (static).
    fn put_numbers(&mut self, argv: &[f64], argn: i32, dst: &mut [u8], dp: &mut usize) {
        todo!()
    }
    /// `get_integer` (static).
    fn get_integer(&mut self, data: &[u8], p: &mut usize) {
        todo!()
    }
    /// `get_longint` (static).
    fn get_longint(&mut self, data: &[u8], p: &mut usize) {
        todo!()
    }
    /// `t1char_build_charpath` (static).
    fn t1char_build_charpath(
        &mut self,
        cd: &mut T1Chardesc,
        data: &[u8],
        p: &mut usize,
        subrs: Option<&CffIndex>,
    ) {
        todo!()
    }
    /// `do_postproc` (static).
    fn do_postproc(&mut self, cd: &mut T1Chardesc) {
        todo!()
    }
    /// `t1char_encode_charpath` (static): bytes written to `dst` (C's
    /// `dst` .. `endptr`).
    fn t1char_encode_charpath(
        &mut self,
        cd: &mut T1Chardesc,
        default_width: f64,
        nominal_width: f64,
        dst: &mut [u8],
    ) -> i32 {
        todo!()
    }
}

impl Dpx {
    /// `t1char_get_metrics`: 0 (metrics into `ginfo`).
    pub fn t1char_get_metrics(
        &mut self,
        src: &[u8],
        subrs: Option<&CffIndex>,
        ginfo: Option<&mut T1Ginfo>,
    ) -> i32 {
        todo!()
    }
    /// `t1char_convert_charstring`: Type 1 `src` as Type 2 into `dst`
    /// (`dstlen` = `dst.len()`); the bytes written.
    pub fn t1char_convert_charstring(
        &mut self,
        dst: &mut [u8],
        src: &[u8],
        subrs: Option<&CffIndex>,
        default_width: f64,
        nominal_width: f64,
        ginfo: Option<&mut T1Ginfo>,
    ) -> i32 {
        todo!()
    }
}
