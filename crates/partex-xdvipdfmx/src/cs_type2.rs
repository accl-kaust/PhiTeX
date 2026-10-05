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

/// `DST_NEED(limit, *dest + n)`.
macro_rules! dst_need {
    ($s:expr, $dst:expr, $dp:expr, $n:expr) => {
        if $dst.len() < *$dp + ($n) as usize {
            $s.status = CS_BUFFER_ERROR;
            return;
        }
    };
}

/// `SRC_NEED(endptr, *data + n)`.
macro_rules! src_need {
    ($s:expr, $data:expr, $p:expr, $n:expr) => {
        if $data.len() < *$p + ($n) as usize {
            $s.status = CS_PARSE_ERROR;
            return;
        }
    };
}

/// `NEED(a, b)`.
macro_rules! need {
    ($s:expr, $a:expr, $b:expr) => {
        if ($a as i32) < ($b as i32) {
            $s.status = CS_STACK_ERROR;
            return;
        }
    };
}

/// `get_subr` (static): subr `id` (bias applied) of `subr_idx`.
fn get_subr(subr_idx: Option<&CffIndex>, id: i32) -> &[u8] {
    let Some(subr_idx) = subr_idx else {
        error!(
            "{}: Subroutine called but no subroutine found.",
            CS_TYPE2_DEBUG_STR
        );
    };

    let count = subr_idx.count;
    let mut id = id;

    // Adding bias number
    if count < 1240 {
        id += 107;
    } else if count < 33900 {
        id += 1131;
    } else {
        id += 32768;
    }

    if id > i32::from(count) {
        error!(
            "{}: Invalid Subr index: {} (max={})",
            CS_TYPE2_DEBUG_STR, id, count
        );
    }

    let id = id as usize;
    let len = subr_idx.offset[id + 1].wrapping_sub(subr_idx.offset[id]) as usize;
    let a = subr_idx.offset[id] as usize - 1;
    &subr_idx.data[a..a + len]
}

impl State {
    /// `clear_stack` (static).
    fn clear_stack(&mut self, dst: &mut [u8], dp: &mut usize) {
        for i in 0..self.stack_top as usize {
            let value = self.arg_stack[i];
            // Nearest integer value
            let mut ivalue = libm::floor(value + 0.5) as i32;
            if value >= 0x8000 as f64 || value <= (-0x8000 - 1) as f64 {
                // This number cannot be represented as a single operand. We
                // must use `a b mul ...' or `a c div' to represent large
                // values.
                error!(
                    "{}: Argument value too large. (This is bug)",
                    CS_TYPE2_DEBUG_STR
                );
            } else if libm::fabs(value - f64::from(ivalue)) > 3.0e-5 {
                // 16.16-bit signed fixed value
                dst_need!(self, dst, dp, 5);
                dst[*dp] = 255;
                ivalue = libm::floor(value) as i32; // mantissa
                dst[*dp + 1] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 2] = (ivalue & 0xff) as u8;
                ivalue = ((value - f64::from(ivalue)) * f64::from(0x10000)) as i32; // fraction
                dst[*dp + 3] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 4] = (ivalue & 0xff) as u8;
                *dp += 5;
                // Everything else are integers.
            } else if (-107..=107).contains(&ivalue) {
                dst_need!(self, dst, dp, 1);
                dst[*dp] = (ivalue + 139) as u8;
                *dp += 1;
            } else if (108..=1131).contains(&ivalue) {
                dst_need!(self, dst, dp, 2);
                ivalue = 0xf700 + ivalue - 108;
                dst[*dp] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 1] = (ivalue & 0xff) as u8;
                *dp += 2;
            } else if (-1131..=-108).contains(&ivalue) {
                dst_need!(self, dst, dp, 2);
                ivalue = 0xfb00 - ivalue - 108;
                dst[*dp] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 1] = (ivalue & 0xff) as u8;
                *dp += 2;
            } else if (-32768..=32767).contains(&ivalue) {
                // shortint
                dst_need!(self, dst, dp, 3);
                dst[*dp] = 28;
                dst[*dp + 1] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 2] = (ivalue & 0xff) as u8;
                *dp += 3;
            } else {
                // Shouldn't come here
                error!("{}: Unexpected error.", CS_TYPE2_DEBUG_STR);
            }
        }

        self.stack_top = 0; // clear stack
    }

    /// `do_operator1` (static).
    ///
    /// phase: 0 inital state; 1 hint declaration, first stack-clearing
    /// operator appeared; 2 in path construction.
    fn do_operator1(&mut self, dst: &mut [u8], dp: &mut usize, data: &[u8], p: &mut usize) {
        let op = data[*p];

        *p += 1;

        match op {
            // charstring may have hintmask if above operator have seen
            CS_HSTEMHM | CS_VSTEMHM | CS_HSTEM | CS_VSTEM => {
                if self.phase == 0 && (self.stack_top % 2) != 0 {
                    self.have_width = 1;
                    self.width = self.arg_stack[0];
                }
                self.num_stems += self.stack_top / 2;
                self.clear_stack(dst, dp);
                dst_need!(self, dst, dp, 1);
                dst[*dp] = op;
                *dp += 1;
                self.phase = 1;
            }
            CS_HINTMASK | CS_CNTRMASK => {
                if self.phase < 2 {
                    if self.phase == 0 && (self.stack_top % 2) != 0 {
                        self.have_width = 1;
                        self.width = self.arg_stack[0];
                    }
                    self.num_stems += self.stack_top / 2;
                }
                self.clear_stack(dst, dp);
                dst_need!(self, dst, dp, 1);
                dst[*dp] = op;
                *dp += 1;
                if self.num_stems > 0 {
                    let masklen = ((self.num_stems + 7) / 8) as usize;
                    dst_need!(self, dst, dp, masklen);
                    src_need!(self, data, p, masklen);
                    dst[*dp..*dp + masklen].copy_from_slice(&data[*p..*p + masklen]);
                    *p += masklen;
                    *dp += masklen;
                }
                self.phase = 2;
            }
            CS_RMOVETO => {
                if self.phase == 0 && (self.stack_top % 2) != 0 {
                    self.have_width = 1;
                    self.width = self.arg_stack[0];
                }
                self.clear_stack(dst, dp);
                dst_need!(self, dst, dp, 1);
                dst[*dp] = op;
                *dp += 1;
                self.phase = 2;
            }
            CS_HMOVETO | CS_VMOVETO => {
                if self.phase == 0 && (self.stack_top % 2) == 0 {
                    self.have_width = 1;
                    self.width = self.arg_stack[0];
                }
                self.clear_stack(dst, dp);
                dst_need!(self, dst, dp, 1);
                dst[*dp] = op;
                *dp += 1;
                self.phase = 2;
            }
            CS_ENDCHAR => {
                if self.stack_top == 1 {
                    self.have_width = 1;
                    self.width = self.arg_stack[0];
                    self.clear_stack(dst, dp);
                } else if self.stack_top == 4 || self.stack_top == 5 {
                    warn!("\"seac\" character deprecated in Type 2 charstring.");
                    self.status = CS_PARSE_ERROR;
                    return;
                } else if self.stack_top > 0 {
                    warn!("{}: Operand stack not empty.", CS_TYPE2_DEBUG_STR);
                }
                dst_need!(self, dst, dp, 1);
                dst[*dp] = op;
                *dp += 1;
                self.status = CS_CHAR_END;
            }
            // above oprators are candidate for first stack-clearing operator
            CS_RLINETO | CS_HLINETO | CS_VLINETO | CS_RRCURVETO | CS_RCURVELINE | CS_RLINECURVE
            | CS_VVCURVETO | CS_HHCURVETO | CS_VHCURVETO | CS_HVCURVETO => {
                if self.phase < 2 {
                    warn!("{}: Broken Type 2 charstring.", CS_TYPE2_DEBUG_STR);
                    self.status = CS_PARSE_ERROR;
                    return;
                }
                self.clear_stack(dst, dp);
                dst_need!(self, dst, dp, 1);
                dst[*dp] = op;
                *dp += 1;
            }
            // all operotors above are stack-clearing operator; no output
            CS_RETURN | CS_CALLGSUBR | CS_CALLSUBR => {
                error!("{}: Unexpected call(g)subr/return", CS_TYPE2_DEBUG_STR);
            }
            _ => {
                // no-op ?
                warn!(
                    "{}: Unknown charstring operator: 0x{:02x}",
                    CS_TYPE2_DEBUG_STR, op
                );
                self.status = CS_PARSE_ERROR;
            }
        }
    }

    /// `do_operator2` (static). `random` is not supported (how random?).
    fn do_operator2(&mut self, dst: &mut [u8], dp: &mut usize, data: &[u8], p: &mut usize) {
        *p += 1;

        src_need!(self, data, p, 1);

        let op = data[*p];
        *p += 1;

        let st = |s: &Self, k: i32| (s.stack_top + k) as usize;

        match op {
            CS_DOTSECTION => {
                // deprecated
                warn!("Operator \"dotsection\" deprecated in Type 2 charstring.");
                self.status = CS_PARSE_ERROR;
            }
            CS_HFLEX | CS_FLEX | CS_HFLEX1 | CS_FLEX1 => {
                if self.phase < 2 {
                    warn!("{}: Broken Type 2 charstring.", CS_TYPE2_DEBUG_STR);
                    self.status = CS_PARSE_ERROR;
                    return;
                }
                self.clear_stack(dst, dp);
                dst_need!(self, dst, dp, 2);
                dst[*dp] = CS_ESCAPE;
                dst[*dp + 1] = op;
                *dp += 2;
            }
            // all operator above are stack-clearing; no output
            CS_AND => {
                need!(self, self.stack_top, 2);
                self.stack_top -= 1;
                if self.arg_stack[st(self, 0)] != 0.0 && self.arg_stack[st(self, -1)] != 0.0 {
                    self.arg_stack[st(self, -1)] = 1.0;
                } else {
                    self.arg_stack[st(self, -1)] = 0.0;
                }
            }
            CS_OR => {
                need!(self, self.stack_top, 2);
                self.stack_top -= 1;
                if self.arg_stack[st(self, 0)] != 0.0 || self.arg_stack[st(self, -1)] != 0.0 {
                    self.arg_stack[st(self, -1)] = 1.0;
                } else {
                    self.arg_stack[st(self, -1)] = 0.0;
                }
            }
            CS_NOT => {
                need!(self, self.stack_top, 1);
                if self.arg_stack[st(self, -1)] != 0.0 {
                    self.arg_stack[st(self, -1)] = 0.0;
                } else {
                    self.arg_stack[st(self, -1)] = 1.0;
                }
            }
            CS_ABS => {
                need!(self, self.stack_top, 1);
                let i = st(self, -1);
                self.arg_stack[i] = libm::fabs(self.arg_stack[i]);
            }
            CS_ADD => {
                need!(self, self.stack_top, 2);
                let (a, b) = (st(self, -2), st(self, -1));
                self.arg_stack[a] += self.arg_stack[b];
                self.stack_top -= 1;
            }
            CS_SUB => {
                need!(self, self.stack_top, 2);
                let (a, b) = (st(self, -2), st(self, -1));
                self.arg_stack[a] -= self.arg_stack[b];
                self.stack_top -= 1;
            }
            CS_DIV => {
                // doesn't check overflow
                need!(self, self.stack_top, 2);
                let (a, b) = (st(self, -2), st(self, -1));
                self.arg_stack[a] /= self.arg_stack[b];
                self.stack_top -= 1;
            }
            CS_NEG => {
                need!(self, self.stack_top, 1);
                let i = st(self, -1);
                self.arg_stack[i] *= -1.0;
            }
            CS_EQ => {
                need!(self, self.stack_top, 2);
                self.stack_top -= 1;
                if self.arg_stack[st(self, 0)] == self.arg_stack[st(self, -1)] {
                    self.arg_stack[st(self, -1)] = 1.0;
                } else {
                    self.arg_stack[st(self, -1)] = 0.0;
                }
            }
            CS_DROP => {
                need!(self, self.stack_top, 1);
                self.stack_top -= 1;
            }
            CS_PUT => {
                need!(self, self.stack_top, 2);
                self.stack_top -= 1;
                let idx = self.arg_stack[st(self, 0)] as i32;
                need!(self, CS_TRANS_ARRAY_MAX, idx);
                self.stack_top -= 1;
                self.trn_array[idx as usize] = self.arg_stack[st(self, 0)];
            }
            CS_GET => {
                need!(self, self.stack_top, 1);
                let idx = self.arg_stack[st(self, -1)] as i32;
                need!(self, CS_TRANS_ARRAY_MAX, idx);
                self.arg_stack[st(self, -1)] = self.trn_array[idx as usize];
            }
            CS_IFELSE => {
                need!(self, self.stack_top, 4);
                self.stack_top -= 3;
                if self.arg_stack[st(self, 1)] > self.arg_stack[st(self, 2)] {
                    self.arg_stack[st(self, -1)] = self.arg_stack[st(self, 0)];
                }
            }
            CS_MUL => {
                need!(self, self.stack_top, 2);
                let (a, b) = (st(self, -2), st(self, -1));
                self.arg_stack[a] = self.arg_stack[a] * self.arg_stack[b];
                self.stack_top -= 1;
            }
            CS_SQRT => {
                need!(self, self.stack_top, 1);
                let i = st(self, -1);
                self.arg_stack[i] = libm::sqrt(self.arg_stack[i]);
            }
            CS_DUP => {
                need!(self, self.stack_top, 1);
                need!(self, CS_ARG_STACK_MAX, self.stack_top + 1);
                self.arg_stack[st(self, 0)] = self.arg_stack[st(self, -1)];
                self.stack_top += 1;
            }
            CS_EXCH => {
                need!(self, self.stack_top, 2);
                let (a, b) = (st(self, -2), st(self, -1));
                self.arg_stack.swap(a, b);
            }
            CS_INDEX => {
                need!(self, self.stack_top, 2); // need two arguments at least
                let idx = self.arg_stack[st(self, -1)] as i32;
                if idx < 0 {
                    self.arg_stack[st(self, -1)] = self.arg_stack[st(self, -2)];
                } else {
                    need!(self, self.stack_top, idx + 2);
                    self.arg_stack[st(self, -1)] = self.arg_stack[st(self, -idx - 2)];
                }
            }
            CS_ROLL => {
                need!(self, self.stack_top, 2);
                self.stack_top -= 1;
                let mut j = self.arg_stack[st(self, 0)] as i32;
                self.stack_top -= 1;
                let n = self.arg_stack[st(self, 0)] as i32;
                need!(self, self.stack_top, n);
                let top = self.stack_top;
                if j > 0 {
                    j %= n;
                    while j > 0 {
                        j -= 1;
                        let save = self.arg_stack[(top - 1) as usize];
                        let mut i = top - 1;
                        while i > top - n {
                            self.arg_stack[i as usize] = self.arg_stack[(i - 1) as usize];
                            i -= 1;
                        }
                        self.arg_stack[i as usize] = save;
                    }
                } else {
                    j = (-j) % n;
                    while j > 0 {
                        j -= 1;
                        let save = self.arg_stack[(top - n) as usize];
                        let mut i = top - n;
                        while i < top - 1 {
                            self.arg_stack[i as usize] = self.arg_stack[(i + 1) as usize];
                            i += 1;
                        }
                        self.arg_stack[i as usize] = save;
                    }
                }
            }
            CS_RANDOM => {
                warn!(
                    "{}: Charstring operator \"random\" found.",
                    CS_TYPE2_DEBUG_STR
                );
                need!(self, CS_ARG_STACK_MAX, self.stack_top + 1);
                self.arg_stack[st(self, 0)] = 1.0;
                self.stack_top += 1;
            }
            _ => {
                // no-op ?
                warn!(
                    "{}: Unknown charstring operator: 0x0c{:02x}",
                    CS_TYPE2_DEBUG_STR, op
                );
                self.status = CS_PARSE_ERROR;
            }
        }
    }

    /// `get_integer` (static): exactly the DICT encoding (except 29).
    fn get_integer(&mut self, data: &[u8], p: &mut usize) {
        let mut result: i32;
        let b0 = data[*p];

        *p += 1;

        if b0 == 28 {
            // shortint
            src_need!(self, data, p, 2);
            let b1 = data[*p];
            let b2 = data[*p + 1];
            result = i32::from(b1) * 256 + i32::from(b2);
            if result > 0x7fff {
                result -= 0x10000;
            }
            *p += 2;
        } else if (32..=246).contains(&b0) {
            // int (1)
            result = i32::from(b0) - 139;
        } else if (247..=250).contains(&b0) {
            // int (2)
            src_need!(self, data, p, 1);
            let b1 = data[*p];
            result = (i32::from(b0) - 247) * 256 + i32::from(b1) + 108;
            *p += 1;
        } else if (251..=254).contains(&b0) {
            src_need!(self, data, p, 1);
            let b1 = data[*p];
            result = -(i32::from(b0) - 251) * 256 - i32::from(b1) - 108;
            *p += 1;
        } else {
            self.status = CS_PARSE_ERROR;
            return;
        }

        need!(self, CS_ARG_STACK_MAX, self.stack_top + 1);
        self.arg_stack[self.stack_top as usize] = f64::from(result);
        self.stack_top += 1;
    }

    /// `get_fixed` (static): a signed 16.16-bit fixed number.
    fn get_fixed(&mut self, data: &[u8], p: &mut usize) {
        *p += 1;

        src_need!(self, data, p, 4);

        let mut ivalue = i32::from(data[*p]) * 0x100 + i32::from(data[*p + 1]);
        let mut rvalue = if ivalue > 0x7fff {
            f64::from(ivalue - 0x10000)
        } else {
            f64::from(ivalue)
        };
        ivalue = i32::from(data[*p + 2]) * 0x100 + i32::from(data[*p + 3]);
        rvalue += f64::from(ivalue) / f64::from(0x10000);

        need!(self, CS_ARG_STACK_MAX, self.stack_top + 1);
        self.arg_stack[self.stack_top as usize] = rvalue;
        self.stack_top += 1;
        *p += 4;
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
        if self.nest > CS_SUBR_NEST_MAX {
            error!("{}: Subroutine nested too deeply.", CS_TYPE2_DEBUG_STR);
        }

        self.nest += 1;

        while *p < data.len() && self.status == CS_PARSE_OK {
            let b0 = data[*p];
            if b0 == 255 {
                // 16-bit.16-bit fixed signed number
                self.get_fixed(data, p);
            } else if b0 == CS_RETURN {
                self.status = CS_SUBR_RETURN;
            } else if b0 == CS_CALLGSUBR {
                if self.stack_top < 1 {
                    self.status = CS_STACK_ERROR;
                } else {
                    self.stack_top -= 1;
                    let subr = get_subr(gsubr_idx, self.arg_stack[self.stack_top as usize] as i32);
                    if *dp + subr.len() > dst.len() {
                        error!("{}: Possible buffer overflow.", CS_TYPE2_DEBUG_STR);
                    }
                    self.do_charstring(dst, dp, subr, &mut 0, gsubr_idx, subr_idx);
                    *p += 1;
                }
            } else if b0 == CS_CALLSUBR {
                if self.stack_top < 1 {
                    self.status = CS_STACK_ERROR;
                } else {
                    self.stack_top -= 1;
                    let subr = get_subr(subr_idx, self.arg_stack[self.stack_top as usize] as i32);
                    if dst.len() < *dp + subr.len() {
                        error!("{}: Possible buffer overflow.", CS_TYPE2_DEBUG_STR);
                    }
                    self.do_charstring(dst, dp, subr, &mut 0, gsubr_idx, subr_idx);
                    *p += 1;
                }
            } else if b0 == CS_ESCAPE {
                self.do_operator2(dst, dp, data, p);
            } else if b0 < 32 && b0 != 28 {
                // 19, 20 need mask
                self.do_operator1(dst, dp, data, p);
            } else if (b0 <= 22 && b0 >= 27) || b0 == 31 {
                // reserved
                self.status = CS_PARSE_ERROR; // not an error ?
            } else {
                // integer
                self.get_integer(data, p);
            }
        }

        if self.status == CS_SUBR_RETURN {
            self.status = CS_PARSE_OK;
        } else if self.status == CS_CHAR_END && *p < data.len() {
            warn!("{}: Garbage after endchar.", CS_TYPE2_DEBUG_STR);
        } else if self.status < CS_PARSE_OK {
            // error
            error!(
                "{}: Parsing charstring failed: (status={}, stack={})",
                CS_TYPE2_DEBUG_STR, self.status, self.stack_top
            );
        }

        self.nest -= 1;
    }

    /// `cs_parse_init` (static).
    fn cs_parse_init(&mut self) {
        self.status = CS_PARSE_OK;
        self.nest = 0;
        self.phase = 0;
        self.num_stems = 0;
        self.stack_top = 0;
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
        let st = &mut self.cs_type2;
        let mut dp = 0usize;
        let mut p = 0usize;

        st.cs_parse_init();

        st.width = 0.0;
        st.have_width = 0;

        // expand call(g)subrs
        st.do_charstring(dst, &mut dp, src, &mut p, gsubr, subr);

        if let Some(ginfo) = ginfo {
            ginfo.flags = 0; // not used
            if st.have_width != 0 {
                ginfo.wx = nominal_width + st.width;
            } else {
                ginfo.wx = default_width;
            }
        }

        dp as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(src: &[u8], gsubr: Option<&CffIndex>) -> (Vec<u8>, f64) {
        let mut st = State::default();
        let mut dst = vec![0u8; 256];
        let (mut dp, mut p) = (0, 0);
        st.cs_parse_init();
        st.do_charstring(&mut dst, &mut dp, src, &mut p, gsubr, None);
        dst.truncate(dp);
        (dst, if st.have_width != 0 { st.width } else { -1.0 })
    }

    #[test]
    fn copy_with_gsubr() {
        // gsubr 0 (biased -107): "10 rlineto return"
        let mut g = CffIndex::cff_new_index(1);
        g.offset[1] = 4;
        g.data = vec![149, CS_RLINETO, CS_RETURN];
        // width 50, 0 0 rmoveto, -107 callgsubr, 1.5 (fixed) 0 rlineto, endchar
        let src = [
            189,
            139,
            139,
            CS_RMOVETO,
            32,
            CS_CALLGSUBR,
            255,
            0,
            1,
            0x80,
            0,
            139,
            CS_RLINETO,
            CS_ENDCHAR,
        ];
        let (out, w) = copy(&src, Some(&g));
        assert_eq!(w, 50.0);
        assert_eq!(
            out,
            [
                189, 139, 139, CS_RMOVETO, 149, CS_RLINETO, 255, 0, 1, 0x80, 0, 139, CS_RLINETO,
                CS_ENDCHAR
            ]
        );
    }

    #[test]
    fn arithmetic_folds() {
        // 0 0 rmoveto 3 4 add 1000 0 rlineto -> rlineto with 7 1000
        let src = [
            139, 139, CS_RMOVETO, 142, 143, CS_ESCAPE, CS_ADD, 0xfa, 0x7c, CS_RLINETO, CS_ENDCHAR,
        ];
        let (out, w) = copy(&src, None);
        assert_eq!(w, -1.0);
        assert_eq!(
            out,
            [
                139, 139, CS_RMOVETO, 146, 0xfa, 0x7c, CS_RLINETO, CS_ENDCHAR
            ]
        );
    }
}
