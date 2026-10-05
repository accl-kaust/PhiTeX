//! t1_char.c, t1_char.h: Type 1 charstrings decoded and re-encoded as
//! Type 2.
//!
//! The C statics are [`State`] (`self.t1_char`); the static functions
//! are its methods. Input `(card8 **data, card8 *endptr)` is
//! `(data: &[u8], p: &mut usize)`; output `(card8 **dest, card8 *limit)`
//! is `(dst: &mut [u8], dp: &mut usize)`. The `t1_cpath` linked list
//! (`charpath` .. `lastpath`) is the `Vec` [`T1Chardesc::charpath`].

use crate::cff::{CS_ARG_STACK_MAX, CS_STEM_ZONE_MAX, CS_SUBR_NEST_MAX, Card8, CffIndex};
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
const CS_HSTEM: i32 = 1;
const CS_VSTEM: i32 = 3;
const CS_VMOVETO: i32 = 4;
const CS_RLINETO: i32 = 5;
const CS_HLINETO: i32 = 6;
const CS_VLINETO: i32 = 7;
const CS_RRCURVETO: i32 = 8;
const CS_CLOSEPATH: i32 = 9;
const CS_CALLSUBR: i32 = 10;
const CS_RETURN: i32 = 11;
const CS_ESCAPE: i32 = 12;
const CS_HSBW: i32 = 13;
const CS_ENDCHAR: i32 = 14;
const CS_HSTEMHM: i32 = 18;
const CS_HINTMASK: i32 = 19;
const CS_CNTRMASK: i32 = 20;
const CS_RMOVETO: i32 = 21;
const CS_HMOVETO: i32 = 22;
const CS_VSTEMHM: i32 = 23;
const CS_RCURVELINE: i32 = 24;
const CS_RLINECURVE: i32 = 25;
const CS_VVCURVETO: i32 = 26;
const CS_HHCURVETO: i32 = 27;
const CS_CALLGSUBR: i32 = 29;
const CS_VHCURVETO: i32 = 30;
const CS_HVCURVETO: i32 = 31;
const CS_DOTSECTION: i32 = 0;
const CS_VSTEM3: i32 = 1;
const CS_HSTEM3: i32 = 2;
const CS_AND: i32 = 3;
const CS_OR: i32 = 4;
const CS_NOT: i32 = 5;
const CS_SEAC: i32 = 6;
const CS_SBW: i32 = 7;
const CS_ABS: i32 = 9;
const CS_ADD: i32 = 10;
const CS_SUB: i32 = 11;
const CS_DIV: i32 = 12;
const CS_NEG: i32 = 14;
const CS_EQ: i32 = 15;
const CS_CALLOTHERSUBR: i32 = 16;
const CS_POP: i32 = 17;
const CS_DROP: i32 = 18;
const CS_PUT: i32 = 20;
const CS_GET: i32 = 21;
const CS_IFELSE: i32 = 22;
const CS_RANDOM: i32 = 23;
const CS_MUL: i32 = 24;
const CS_SQRT: i32 = 26;
const CS_DUP: i32 = 27;
const CS_EXCH: i32 = 28;
const CS_INDEX: i32 = 29;
const CS_ROLL: i32 = 30;
const CS_SETCURRENTPOINT: i32 = 33;
const CS_HFLEX: i32 = 34;
const CS_FLEX: i32 = 35;
const CS_HFLEX1: i32 = 36;
const CS_FLEX1: i32 = 37;

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

/// `IS_PATH_OPERATOR`.
#[must_use]
pub fn is_path_operator(o: i32) -> bool {
    (o >= CS_VMOVETO && o <= CS_CLOSEPATH)
        || (o >= CS_RMOVETO && o <= CS_HVCURVETO && o != CS_VSTEMHM && o != CS_CALLGSUBR && o != 28)
}

/// `stem_compare` (static): qsort order of two stems.
fn stem_compare(s1: &T1Stem, s2: &T1Stem) -> i32 {
    if s1.dir == s2.dir {
        if s1.pos == s2.pos {
            if s1.del == s2.del {
                0
            } else if s1.del < s2.del {
                -1
            } else {
                1
            }
        } else if s1.pos < s2.pos {
            -1
        } else {
            1
        }
    } else if s1.dir == HSTEM {
        -1
    } else {
        1
    }
}

/// `SORT_STEMS(cd)`: there are no equal stems (`add_stem` merges them),
/// so any sort gives qsort's order.
fn sort_stems(cd: &mut T1Chardesc) {
    let n = cd.num_stems as usize;
    cd.stems[..n].sort_by(|a, b| stem_compare(a, b).cmp(&0));
}

/// `get_stem` (static): the index of stem `stem_id`, or -1.
fn get_stem(cd: &T1Chardesc, stem_id: i32) -> i32 {
    let mut i = 0;
    while i < cd.num_stems {
        if cd.stems[i as usize].id == stem_id {
            break;
        }
        i += 1;
    }
    if i < cd.num_stems { i } else { -1 }
}

/// `add_stem` (static): the stem's id, or -1.
fn add_stem(cd: &mut T1Chardesc, pos: f64, del: f64, dir: i32) -> i32 {
    let pos = pos + if dir == HSTEM { cd.sbw.sby } else { cd.sbw.sbx };
    let mut i = 0;
    while i < cd.num_stems as usize {
        if cd.stems[i].dir == dir && cd.stems[i].pos == pos && cd.stems[i].del == del {
            break;
        }
        i += 1;
    }
    if i == cd.num_stems as usize {
        if cd.num_stems as usize == CS_STEM_ZONE_MAX {
            return -1;
        }
        cd.stems[i].dir = dir;
        cd.stems[i].pos = pos;
        cd.stems[i].del = del;
        cd.stems[i].id = cd.num_stems;
        cd.num_stems += 1;
    }
    cd.stems[i].id
}

/// `copy_args` (static).
fn copy_args(args1: &mut [f64], args2: &[f64], count: i32) {
    for i in 0..count.max(0) as usize {
        args1[i] = args2[i];
    }
}

/// `init_charpath` (static).
fn init_charpath(cd: &mut T1Chardesc) {
    cd.flags = T1_CS_FLAG_NONE;
    cd.num_stems = 0;
    cd.sbw.wx = 0.0;
    cd.sbw.wy = 0.0;
    cd.sbw.sbx = 0.0;
    cd.sbw.sby = 0.0;
    cd.bbox.llx = 0.0;
    cd.bbox.lly = 0.0;
    cd.bbox.urx = 0.0;
    cd.bbox.ury = 0.0;
    cd.charpath = Vec::new();
}

/// `release_charpath` (static).
fn release_charpath(cd: &mut T1Chardesc) {
    cd.charpath = Vec::new();
}

/// `UPDATE_BBOX(b, x, y)`.
fn update_bbox(b: &mut GlyphBbox, x: f64, y: f64) {
    if b.llx > x {
        b.llx = x;
    }
    if b.urx < x {
        b.urx = x;
    }
    if b.lly > y {
        b.lly = y;
    }
    if b.ury < y {
        b.ury = y;
    }
}

/// Appends `cur`'s arguments to `prev`'s (the merge of `do_postproc`).
fn append_args(prev: &mut T1Cpath, cur: &T1Cpath) {
    let n = prev.num_args as usize;
    copy_args(&mut prev.args[n..], &cur.args, cur.num_args);
    prev.num_args += cur.num_args;
}

impl State {
    /// `RESET_STATE()`.
    fn reset_state(&mut self) {
        self.status = CS_PARSE_OK;
        self.phase = T1_CS_PHASE_INIT;
        self.nest = 0;
        self.ps_stack_top = 0;
    }
    /// `add_charpath` (static; a method: it sets `phase`).
    fn add_charpath(&mut self, cd: &mut T1Chardesc, type_: i32, argv: &[f64], argn: i32) {
        assert!(argn as usize <= CS_ARG_STACK_MAX);
        let mut p = T1Cpath {
            type_,
            num_args: argn,
            ..T1Cpath::default()
        };
        let mut argn = argn;
        while argn > 0 {
            argn -= 1;
            p.args[argn as usize] = argv[argn as usize];
        }
        cd.charpath.push(p);

        if type_ >= 0 && self.phase != T1_CS_PHASE_FLEX && is_path_operator(type_) {
            self.phase = T1_CS_PHASE_PATH;
        }
    }
    /// `ADD_PATH(cd, t, n)`: the top `n` operands.
    fn add_path(&mut self, cd: &mut T1Chardesc, t: i32, n: i32) {
        let top = self.cs_stack_top as usize;
        let args = self.cs_arg_stack;
        self.add_charpath(cd, t, &args[top - n as usize..top], n);
    }
    /// `CHECKSTACK(n)`: false (and the status set) if fewer than `n`.
    fn checkstack(&mut self, n: i32) -> bool {
        if self.cs_stack_top < n {
            self.status = CS_STACK_ERROR;
            return false;
        }
        true
    }
    /// `cs_arg_stack[--cs_stack_top]`.
    fn pop(&mut self) -> f64 {
        self.cs_stack_top -= 1;
        self.cs_arg_stack[self.cs_stack_top as usize]
    }
    /// `cs_arg_stack[cs_stack_top++] = v`.
    fn push(&mut self, v: f64) {
        self.cs_arg_stack[self.cs_stack_top as usize] = v;
        self.cs_stack_top += 1;
    }
    /// `ps_arg_stack[--ps_stack_top]`.
    fn ps_pop(&mut self) -> f64 {
        self.ps_stack_top -= 1;
        self.ps_arg_stack[self.ps_stack_top as usize]
    }
    /// `do_operator1` (static).
    fn do_operator1(&mut self, cd: &mut T1Chardesc, data: &[u8], p: &mut usize) {
        let mut op = i32::from(data[*p]);

        *p += 1;

        match op {
            CS_CLOSEPATH => {
                // From T1 spec.:
                //  Note that, unlike the closepath command in the PostScript
                //  language, this command does not reposition the current
                //  point. Any subsequent rmoveto must be relative to the
                //  current point in force before the Type 1 font format
                //  closepath command was given.
                // noop
                self.cs_stack_top = 0;
            }
            CS_HSBW => {
                if !self.checkstack(2) {
                    return;
                }
                cd.sbw.wx = self.pop();
                cd.sbw.wy = 0.0;
                cd.sbw.sbx = self.pop();
                cd.sbw.sby = 0.0;
                self.cs_stack_top = 0;
                // hsbw does NOT set currentpoint.
            }
            CS_HSTEM | CS_VSTEM => {
                if !self.checkstack(2) {
                    return;
                }
                let top = self.cs_stack_top as usize;
                let stem_id = add_stem(
                    cd,
                    self.cs_arg_stack[top - 2],
                    self.cs_arg_stack[top - 1],
                    if op == CS_HSTEM { HSTEM } else { VSTEM },
                );
                if stem_id < 0 {
                    warn!("Too many hints...");
                    self.status = CS_PARSE_ERROR;
                    return;
                }
                // Put stem_id onto the stack...
                self.push(f64::from(stem_id));
                self.add_path(cd, CS_HINT_DECL, 1);
                self.cs_stack_top = 0;
            }
            CS_RMOVETO => {
                // Reference point is (0, 0) in Type 2 charstring.
                if !self.checkstack(2) {
                    return;
                }
                if self.phase < T1_CS_PHASE_PATH {
                    let top = self.cs_stack_top as usize;
                    self.cs_arg_stack[top - 2] += cd.sbw.sbx;
                    self.cs_arg_stack[top - 1] += cd.sbw.sby;
                }
                self.add_path(cd, op, 2);
                self.cs_stack_top = 0;
            }
            CS_HMOVETO | CS_VMOVETO => {
                if !self.checkstack(1) {
                    return;
                }
                let mut argn = 1;
                if self.phase < T1_CS_PHASE_PATH {
                    // The reference point for the first moveto operator is
                    // diferrent between Type 1 charstring and Type 2
                    // charstring. We compensate it.
                    let top = self.cs_stack_top as usize;
                    if op == CS_HMOVETO {
                        self.cs_arg_stack[top - 1] += cd.sbw.sbx;
                        if cd.sbw.sby != 0.0 {
                            self.push(cd.sbw.sby);
                            argn = 2;
                            op = CS_RMOVETO;
                        }
                    } else {
                        self.cs_arg_stack[top - 1] += cd.sbw.sby;
                        if cd.sbw.sbx != 0.0 {
                            self.cs_arg_stack[top] = self.cs_arg_stack[top - 1];
                            self.cs_arg_stack[top - 1] = cd.sbw.sbx;
                            self.cs_stack_top += 1;
                            argn = 2;
                            op = CS_RMOVETO;
                        }
                    }
                }
                self.add_path(cd, op, argn);
                self.cs_stack_top = 0;
            }
            CS_ENDCHAR => {
                self.status = CS_CHAR_END;
                self.cs_stack_top = 0;
            }
            // above oprators are candidate for first stack-clearing operator
            CS_RLINETO => {
                if !self.checkstack(2) {
                    return;
                }
                self.add_path(cd, op, 2);
                self.cs_stack_top = 0;
            }
            CS_HLINETO | CS_VLINETO => {
                if !self.checkstack(1) {
                    return;
                }
                self.add_path(cd, op, 1);
                self.cs_stack_top = 0;
            }
            CS_RRCURVETO => {
                if !self.checkstack(6) {
                    return;
                }
                self.add_path(cd, op, 6);
                self.cs_stack_top = 0;
            }
            CS_VHCURVETO | CS_HVCURVETO => {
                if !self.checkstack(4) {
                    return;
                }
                self.add_path(cd, op, 4);
                self.cs_stack_top = 0;
            }
            // all operotors above are stack-clearing operator
            // no output
            CS_RETURN => {}
            CS_CALLSUBR => {
                error!("Unexpected callsubr.");
            }
            _ => {
                // no-op ?
                warn!("Unknown charstring operator: 0x{:02x}", op);
                self.status = CS_PARSE_ERROR;
            }
        }
    }
    /// `do_othersubr0` (static): six control points marked as
    /// `CS_FLEX_CTRL` made a flex path.
    fn do_othersubr0(&mut self, cd: &mut T1Chardesc) {
        if self.ps_stack_top < 1 {
            self.status = CS_PARSE_ERROR;
            return;
        }

        // Seek first CS_FLEX_CTRL mark
        let Some(fi) = cd.charpath.iter().position(|c| c.type_ == CS_FLEX_CTRL) else {
            // C dereferences NULL here.
            self.status = CS_PARSE_ERROR;
            return;
        };
        let mut cur = fi + 1;
        for i in 1..7 {
            if cur >= cd.charpath.len()
                || cd.charpath[cur].type_ != CS_FLEX_CTRL
                || cd.charpath[cur].num_args != 2
            {
                self.status = CS_PARSE_ERROR;
                return;
            }
            let (a0, a1) = (cd.charpath[cur].args[0], cd.charpath[cur].args[1]);
            let flex = &mut cd.charpath[fi];
            if i == 1 {
                flex.args[0] += a0;
                flex.args[1] += a1;
            } else {
                flex.args[2 * i - 2] = a0;
                flex.args[2 * i - 1] = a1;
            }
            cur += 1;
        }
        if cur < cd.charpath.len() {
            self.status = CS_PARSE_ERROR;
            return;
        }
        // Now 'flex' have all six control points, the first pair is
        // relative from starting point.
        cd.charpath.truncate(fi + 1);
        let depth = self.ps_pop(); // flex depth
        let flex = &mut cd.charpath[fi];
        flex.type_ = CS_FLEX;
        flex.args[12] = depth;
        flex.num_args = 13;

        self.phase = T1_CS_PHASE_PATH;
    }
    /// `do_othersubr1` (static): start flex.
    fn do_othersubr1(&mut self) {
        self.phase = T1_CS_PHASE_FLEX;
    }
    /// `do_othersubr2` (static): mark flex control point.
    fn do_othersubr2(&mut self, cd: &mut T1Chardesc) {
        if self.phase != T1_CS_PHASE_FLEX || cd.charpath.is_empty() {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let last = cd.charpath.last_mut().unwrap();
        match last.type_ {
            CS_RMOVETO => {}
            CS_HMOVETO => {
                last.num_args = 2;
                last.args[1] = 0.0;
            }
            CS_VMOVETO => {
                last.num_args = 2;
                last.args[1] = last.args[0];
                last.args[0] = 0.0;
            }
            _ => {
                self.status = CS_PARSE_ERROR;
                return;
            }
        }
        last.type_ = CS_FLEX_CTRL;
    }
    /// `do_othersubr3` (static): hint replacement.
    fn do_othersubr3(&mut self, cd: &mut T1Chardesc) {
        cd.flags |= T1_CS_FLAG_USE_HINTMASK;
    }
    /// `do_othersubr12` (static).
    fn do_othersubr12(&mut self) {
        // Othersubr12 call must immediately follow the hsbw or sbw.
        if self.phase != T1_CS_PHASE_INIT {
            self.status = CS_PARSE_ERROR;
        }
        // noop
    }
    /// `do_othersubr13` (static).
    fn do_othersubr13(&mut self, cd: &mut T1Chardesc) {
        // After #12 callothersubr or hsbw or sbw.
        if self.phase != T1_CS_PHASE_INIT {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let mut stemgroups: Vec<T1Stemgroup> = vec![T1Stemgroup::default(); CS_STEM_GROUP_MAX];

        let num_hgroups = self.ps_pop() as i32;
        if num_hgroups < 0 || num_hgroups > CS_STEM_GROUP_MAX as i32 {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let mut n: i32 = 0;
        let mut pos = 0.0;
        while self.ps_stack_top >= 2 && n < num_hgroups {
            // add_stem() add sidebearing
            pos += self.ps_pop();
            let del = self.ps_pop();
            let stem_id = add_stem(
                cd,
                if del < 0.0 { pos + del } else { pos },
                if del < 0.0 { -del } else { del },
                HSTEM,
            );
            let g = &mut stemgroups[n as usize];
            g.stems[g.num_stems as usize] = f64::from(stem_id);
            g.num_stems += 1;
            pos += del;
            if del < 0.0 {
                pos = 0.0;
                n += 1;
            }
        }
        if n != num_hgroups {
            self.status = CS_STACK_ERROR;
            return;
        }

        let num_vgroups = self.ps_pop() as i32;
        if num_vgroups < 0 || num_vgroups > CS_STEM_GROUP_MAX as i32 {
            self.status = CS_PARSE_ERROR;
            return;
        }
        n = 0;
        pos = 0.0;
        while self.ps_stack_top >= 2 && n < num_vgroups {
            // add_stem() add sidebearing
            pos += self.ps_pop();
            let del = self.ps_pop();
            let stem_id = add_stem(
                cd,
                if del < 0.0 { pos + del } else { pos },
                if del < 0.0 { -del } else { del },
                VSTEM,
            );
            let g = &mut stemgroups[n as usize];
            g.stems[g.num_stems as usize] = f64::from(stem_id);
            g.num_stems += 1;
            pos += del;
            if del < 0.0 {
                pos = 0.0;
                n += 1;
            }
        }
        if n != num_vgroups {
            self.status = CS_STACK_ERROR;
            return;
        }

        for n in 0..num_hgroups.max(num_vgroups) as usize {
            let g = &stemgroups[n];
            self.add_charpath(cd, CS_CNTRMASK, &g.stems, g.num_stems);
        }

        cd.flags |= T1_CS_FLAG_USE_CNTRMASK;
    }
    /// `do_callothersubr` (static).
    fn do_callothersubr(&mut self, cd: &mut T1Chardesc) {
        if !self.checkstack(2) {
            return;
        }
        let subrno = self.pop() as i32;
        let mut argn = self.pop() as i32;

        if !self.checkstack(argn) {
            return;
        }
        if self.ps_stack_top + argn > PS_ARG_STACK_MAX as i32 {
            self.status = CS_PARSE_ERROR;
            return;
        }
        while argn > 0 {
            argn -= 1;
            let v = self.pop();
            self.ps_arg_stack[self.ps_stack_top as usize] = v;
            self.ps_stack_top += 1;
        }

        match subrno {
            0 => self.do_othersubr0(cd),
            1 => self.do_othersubr1(),
            2 => self.do_othersubr2(cd),
            3 => self.do_othersubr3(cd),
            12 => self.do_othersubr12(),
            13 => self.do_othersubr13(cd),
            _ => error!("Unknown othersubr #{}.", subrno),
        }
    }
    /// `do_operator2` (static).
    fn do_operator2(&mut self, cd: &mut T1Chardesc, data: &[u8], p: &mut usize) {
        *p += 1;

        // SRC_NEED(endptr, *data + 1)
        if data.len() < *p + 1 {
            self.status = CS_PARSE_ERROR;
            return;
        }

        let op = i32::from(data[*p]);
        *p += 1;

        match op {
            CS_SBW => {
                if !self.checkstack(4) {
                    return;
                }
                cd.sbw.wy = self.pop();
                cd.sbw.wx = self.pop();
                cd.sbw.sby = self.pop();
                cd.sbw.sbx = self.pop();
                self.cs_stack_top = 0;
            }
            CS_HSTEM3 | CS_VSTEM3 => {
                // TODO:
                //  The counter control can be used for hstem3 and vstem3
                //  operator if LanguageGroup is not equal to 1.
                if !self.checkstack(6) {
                    return;
                }
                for i in (0..=2usize).rev() {
                    let top = self.cs_stack_top as usize;
                    let stem_id = add_stem(
                        cd,
                        self.cs_arg_stack[top - 2 * i - 2],
                        self.cs_arg_stack[top - 2 * i - 1],
                        if op == CS_HSTEM3 { HSTEM } else { VSTEM },
                    );
                    if stem_id < 0 {
                        warn!("Too many hints...");
                        self.status = CS_PARSE_ERROR;
                        return;
                    }
                    // Put stem_id onto the stack...
                    self.push(f64::from(stem_id));
                    self.add_path(cd, CS_HINT_DECL, 1);
                    self.cs_stack_top -= 1;
                }
                self.cs_stack_top = 0;
            }
            CS_SETCURRENTPOINT => {
                if !self.checkstack(2) {
                    return;
                }
                // noop
                self.cs_stack_top = 0;
            }
            // all operator above are stack-clearing
            CS_POP => {
                // Transfer a operand from PS interpreter operand stack to
                // BuildChar operand stack.
                if self.ps_stack_top < 1 {
                    self.status = CS_PARSE_ERROR;
                    return;
                }
                // LIMITCHECK(1)
                if self.cs_stack_top + 1 > CS_ARG_STACK_MAX as i32 {
                    self.status = CS_STACK_ERROR;
                    return;
                }
                let v = self.ps_pop();
                self.push(v);
            }
            CS_DOTSECTION => {
                // noop
            }
            CS_DIV => {
                // TODO: check overflow
                if !self.checkstack(2) {
                    return;
                }
                let top = self.cs_stack_top as usize;
                self.cs_arg_stack[top - 2] /= self.cs_arg_stack[top - 1];
                self.cs_stack_top -= 1;
            }
            CS_CALLOTHERSUBR => {
                self.do_callothersubr(cd);
            }
            CS_SEAC => {
                if !self.checkstack(5) {
                    return;
                }
                cd.flags |= T1_CS_FLAG_USE_SEAC;
                cd.seac.achar = self.pop() as i32 as Card8;
                cd.seac.bchar = self.pop() as i32 as Card8;
                cd.seac.ady = self.pop();
                cd.seac.adx = self.pop();
                // We must compensate the difference of the glyph origin.
                cd.seac.ady += cd.sbw.sby;
                let asb = self.pop();
                cd.seac.adx += cd.sbw.sbx - asb;
                self.cs_stack_top = 0;
            }
            _ => {
                // no-op ?
                warn!("Unknown charstring operator: 0x0c{:02x}", op);
                self.status = CS_PARSE_ERROR;
            }
        }
    }
    /// `put_numbers` (static): Type 2 5-bytes encoding used.
    fn put_numbers(&mut self, argv: &[f64], argn: i32, dst: &mut [u8], dp: &mut usize) {
        let limit = dst.len();
        for i in 0..argn.max(0) as usize {
            let value = argv[i];
            // Nearest integer value
            let mut ivalue = libm::floor(value + 0.5) as i32;
            if value >= 32768.0 || value <= -32769.0 {
                // This number cannot be represented as a single operand.
                // We must use `a b mul ...' or `a c div' to represent large
                // values.
                error!("Argument value too large. (This is bug)");
            } else if libm::fabs(value - f64::from(ivalue)) > 3.0e-5 {
                // 16.16-bit signed fixed value
                if limit < *dp + 5 {
                    self.status = CS_BUFFER_ERROR;
                    return;
                }
                dst[*dp] = 255;
                ivalue = libm::floor(value) as i32; // mantissa
                dst[*dp + 1] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 2] = (ivalue & 0xff) as u8;
                ivalue = ((value - f64::from(ivalue)) * 65536.0) as i32; // fraction
                dst[*dp + 3] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 4] = (ivalue & 0xff) as u8;
                *dp += 5;
                // Everything else are integers.
            } else if (-107..=107).contains(&ivalue) {
                if limit < *dp + 1 {
                    self.status = CS_BUFFER_ERROR;
                    return;
                }
                dst[*dp] = (ivalue + 139) as u8;
                *dp += 1;
            } else if (108..=1131).contains(&ivalue) {
                if limit < *dp + 2 {
                    self.status = CS_BUFFER_ERROR;
                    return;
                }
                ivalue = 0xf700 + ivalue - 108;
                dst[*dp] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 1] = (ivalue & 0xff) as u8;
                *dp += 2;
            } else if (-1131..=-108).contains(&ivalue) {
                if limit < *dp + 2 {
                    self.status = CS_BUFFER_ERROR;
                    return;
                }
                ivalue = 0xfb00 - ivalue - 108;
                dst[*dp] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 1] = (ivalue & 0xff) as u8;
                *dp += 2;
            } else if (-32768..=32767).contains(&ivalue) {
                // shortint
                if limit < *dp + 3 {
                    self.status = CS_BUFFER_ERROR;
                    return;
                }
                dst[*dp] = 28;
                dst[*dp + 1] = ((ivalue >> 8) & 0xff) as u8;
                dst[*dp + 2] = (ivalue & 0xff) as u8;
                *dp += 3;
            } else {
                // Shouldn't come here
                error!("Unexpected error.");
            }
        }
    }
    /// `get_integer` (static).
    fn get_integer(&mut self, data: &[u8], p: &mut usize) {
        let b0 = i32::from(data[*p]);
        let result: i32;

        *p += 1;

        if b0 == 28 {
            // shortint
            if data.len() < *p + 2 {
                self.status = CS_PARSE_ERROR;
                return;
            }
            let b1 = i32::from(data[*p]);
            let b2 = i32::from(data[*p + 1]);
            let mut r = b1 * 256 + b2;
            if r > 0x7fff {
                r -= 0x10000;
            }
            result = r;
            *p += 2;
        } else if (32..=246).contains(&b0) {
            // int (1)
            result = b0 - 139;
        } else if (247..=250).contains(&b0) {
            // int (2)
            if data.len() < *p + 1 {
                self.status = CS_PARSE_ERROR;
                return;
            }
            let b1 = i32::from(data[*p]);
            result = (b0 - 247) * 256 + b1 + 108;
            *p += 1;
        } else if (251..=254).contains(&b0) {
            if data.len() < *p + 1 {
                self.status = CS_PARSE_ERROR;
                return;
            }
            let b1 = i32::from(data[*p]);
            result = -(b0 - 251) * 256 - b1 - 108;
            *p += 1;
        } else {
            self.status = CS_PARSE_ERROR;
            return;
        }

        // LIMITCHECK(1)
        if self.cs_stack_top + 1 > CS_ARG_STACK_MAX as i32 {
            self.status = CS_STACK_ERROR;
            return;
        }
        self.push(f64::from(result));
    }
    /// `get_longint` (static): Type 1.
    fn get_longint(&mut self, data: &[u8], p: &mut usize) {
        *p += 1;
        if data.len() < *p + 4 {
            self.status = CS_PARSE_ERROR;
            return;
        }
        let mut result = i32::from(data[*p]);
        if result >= 0x80 {
            result -= 0x100;
        }
        *p += 1;
        for _ in 1..4 {
            result = result.wrapping_mul(256).wrapping_add(i32::from(data[*p]));
            *p += 1;
        }

        // LIMITCHECK(1)
        if self.cs_stack_top + 1 > CS_ARG_STACK_MAX as i32 {
            self.status = CS_STACK_ERROR;
            return;
        }
        self.push(f64::from(result));
    }
    /// `t1char_build_charpath` (static): parse charstring and build
    /// charpath.
    fn t1char_build_charpath(
        &mut self,
        cd: &mut T1Chardesc,
        data: &[u8],
        p: &mut usize,
        subrs: Option<&CffIndex>,
    ) {
        if self.nest > CS_SUBR_NEST_MAX {
            error!("Subroutine nested too deeply.");
        }

        self.nest += 1;
        while *p < data.len() && self.status == CS_PARSE_OK {
            let b0 = i32::from(data[*p]);
            if b0 == 255 {
                self.get_longint(data, p); // Type 1
            } else if b0 == CS_RETURN {
                self.status = CS_SUBR_RETURN;
            } else if b0 == CS_CALLSUBR {
                if self.cs_stack_top < 1 {
                    self.status = CS_STACK_ERROR;
                } else {
                    let idx = self.pop() as i32;
                    let Some(subrs) = subrs.filter(|s| idx < i32::from(s.count)) else {
                        error!("Invalid Subr#.");
                    };
                    let idx = idx as usize;
                    let start = subrs.offset[idx] as usize - 1;
                    let len = (subrs.offset[idx + 1] - subrs.offset[idx]) as usize;
                    let subr = &subrs.data[start..start + len];
                    let mut sp = 0;
                    self.t1char_build_charpath(cd, subr, &mut sp, Some(subrs));
                    *p += 1;
                }
            } else if b0 == CS_ESCAPE {
                self.do_operator2(cd, data, p);
            } else if b0 < 32 && b0 != 28 {
                // 19, 20 need mask
                self.do_operator1(cd, data, p);
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
            if !(*p == data.len() - 1 && i32::from(data[*p]) == CS_RETURN) {
                warn!("Garbage after endchar. ({} bytes)", data.len() - *p);
            }
        } else if self.status < CS_PARSE_OK {
            // error
            error!(
                "Parsing charstring failed: (status={}, stack={})",
                self.status, self.cs_stack_top
            );
        }

        self.nest -= 1;
    }
    /// `do_postproc` (static): calculate BoundingBox and compress path.
    fn do_postproc(&mut self, cd: &mut T1Chardesc) {
        if cd.charpath.is_empty() {
            return;
        }

        // Set dummy large value.
        cd.bbox.llx = 100000.0;
        cd.bbox.lly = 100000.0;
        cd.bbox.urx = -100000.0;
        cd.bbox.ury = -100000.0;

        let path = core::mem::take(&mut cd.charpath);
        // `prev` is the last element kept.
        let mut out: Vec<T1Cpath> = Vec::with_capacity(path.len());
        let mut x = 0.0;
        let mut y = 0.0;

        // TRY_COMPACT
        fn try_compact(out: &[T1Cpath], cur: &T1Cpath) -> bool {
            out.last()
                .is_some_and(|prev| (prev.num_args + cur.num_args) < CS_ARG_STACK_MAX as i32)
        }

        for mut cur in path {
            let mut merged = false;
            match cur.type_ {
                CS_RMOVETO => {
                    x += cur.args[0];
                    y += cur.args[1];
                    update_bbox(&mut cd.bbox, x, y);
                }
                CS_RLINETO => {
                    x += cur.args[0];
                    y += cur.args[1];
                    update_bbox(&mut cd.bbox, x, y);
                    if try_compact(&out, &cur) {
                        let prev = out.last_mut().unwrap();
                        if prev.type_ == CS_RLINETO {
                            append_args(prev, &cur);
                            merged = true;
                        } else if prev.type_ == CS_RRCURVETO {
                            append_args(prev, &cur);
                            prev.type_ = CS_RCURVELINE;
                            merged = true;
                        }
                    }
                }
                CS_HMOVETO => {
                    x += cur.args[0];
                    update_bbox(&mut cd.bbox, x, y);
                }
                CS_HLINETO => {
                    x += cur.args[0];
                    update_bbox(&mut cd.bbox, x, y);
                    if try_compact(&out, &cur) {
                        let prev = out.last_mut().unwrap();
                        if (prev.type_ == CS_VLINETO && (prev.num_args % 2) == 1)
                            || (prev.type_ == CS_HLINETO && (prev.num_args % 2) == 0)
                        {
                            append_args(prev, &cur);
                            merged = true;
                        }
                    }
                }
                CS_VMOVETO => {
                    y += cur.args[0];
                    update_bbox(&mut cd.bbox, x, y);
                }
                CS_VLINETO => {
                    y += cur.args[0];
                    update_bbox(&mut cd.bbox, x, y);
                    if try_compact(&out, &cur) {
                        let prev = out.last_mut().unwrap();
                        if (prev.type_ == CS_HLINETO && (prev.num_args % 2) == 1)
                            || (prev.type_ == CS_VLINETO && (prev.num_args % 2) == 0)
                        {
                            append_args(prev, &cur);
                            merged = true;
                        }
                    }
                }
                CS_RRCURVETO => {
                    for i in 0..3 {
                        x += cur.args[2 * i];
                        y += cur.args[2 * i + 1];
                        update_bbox(&mut cd.bbox, x, y);
                    }
                    if try_compact(&out, &cur) {
                        let prev = out.last_mut().unwrap();
                        if prev.type_ == CS_RRCURVETO {
                            append_args(prev, &cur);
                            merged = true;
                        } else if prev.type_ == CS_RLINETO {
                            append_args(prev, &cur);
                            prev.type_ = CS_RLINECURVE;
                            merged = true;
                        }
                    }
                }
                CS_VHCURVETO => {
                    y += cur.args[0];
                    update_bbox(&mut cd.bbox, x, y);
                    x += cur.args[1];
                    y += cur.args[2];
                    update_bbox(&mut cd.bbox, x, y);
                    x += cur.args[3];
                    update_bbox(&mut cd.bbox, x, y);
                    if try_compact(&out, &cur) {
                        let prev = out.last_mut().unwrap();
                        if (prev.type_ == CS_HVCURVETO && ((prev.num_args / 4) % 2) == 1)
                            || (prev.type_ == CS_VHCURVETO && ((prev.num_args / 4) % 2) == 0)
                        {
                            append_args(prev, &cur);
                            merged = true;
                        }
                    }
                }
                CS_HVCURVETO => {
                    x += cur.args[0];
                    update_bbox(&mut cd.bbox, x, y);
                    x += cur.args[1];
                    y += cur.args[2];
                    update_bbox(&mut cd.bbox, x, y);
                    y += cur.args[3];
                    update_bbox(&mut cd.bbox, x, y);
                    if try_compact(&out, &cur) {
                        let prev = out.last_mut().unwrap();
                        if (prev.type_ == CS_VHCURVETO && ((prev.num_args / 4) % 2) == 1)
                            || (prev.type_ == CS_HVCURVETO && ((prev.num_args / 4) % 2) == 0)
                        {
                            append_args(prev, &cur);
                            merged = true;
                        }
                    }
                }
                CS_FLEX => {
                    for i in 0..6 {
                        // C's typo: args[2*1+1], not args[2*i+1].
                        x += cur.args[2 * i];
                        y += cur.args[2 * 1 + 1];
                        update_bbox(&mut cd.bbox, x, y);
                    }
                    if cur.args[12] == 50.0 {
                        if cur.args[1] == 0.0
                            && cur.args[11] == 0.0
                            && cur.args[5] == 0.0
                            && cur.args[7] == 0.0
                            && cur.args[3] + cur.args[9] == 0.0
                        {
                            // cur->args[0] = cur->args[0];  dx1
                            cur.args[1] = cur.args[2]; // dx2
                            cur.args[2] = cur.args[3]; // dy2
                            cur.args[3] = cur.args[4]; // dx3
                            cur.args[4] = cur.args[6]; // dx4
                            cur.args[5] = cur.args[8]; // dx5
                            cur.args[6] = cur.args[10]; // dx6
                            cur.num_args = 7;
                            cur.type_ = CS_HFLEX;
                        } else if cur.args[5] == 0.0
                            && cur.args[7] == 0.0
                            && (cur.args[1] + cur.args[3] + cur.args[9] + cur.args[11]) == 0.0
                        {
                            // args[0..5]: dx1 dy1 dx2 dy2 dx3
                            cur.args[5] = cur.args[6]; // dx4
                            cur.args[6] = cur.args[8]; // dx5
                            cur.args[7] = cur.args[9]; // dy5
                            cur.args[8] = cur.args[10]; // dx6
                            cur.num_args = 9;
                            cur.type_ = CS_HFLEX1;
                        }
                    }
                }
                CS_HINT_DECL | CS_CNTRMASK => {
                    // noop
                }
                _ => {
                    error!("Unexpected Type 2 charstring command {}.", cur.type_);
                }
            }
            if !merged {
                out.push(cur);
            }
        }
        cd.charpath = out;

        // Had no path. Fix lower-left point.
        if cd.bbox.llx > cd.bbox.urx {
            cd.bbox.llx = cd.sbw.wx;
            cd.bbox.urx = cd.sbw.wx;
        }
        if cd.bbox.lly > cd.bbox.ury {
            cd.bbox.lly = cd.sbw.wy;
            cd.bbox.ury = cd.sbw.wy;
        }
    }
    /// `t1char_encode_charpath` (static): the charpath as a Type 2
    /// charstring; bytes written to `dst` (C's `dst` .. `endptr`).
    fn t1char_encode_charpath(
        &mut self,
        cd: &mut T1Chardesc,
        default_width: f64,
        nominal_width: f64,
        dst: &mut [u8],
    ) -> i32 {
        let endptr = dst.len();
        let mut dp: usize = 0;
        // CHECK_BUFFER(n)
        let check_buffer = |dp: usize, n: usize| {
            if dp + n >= endptr {
                error!("Buffer overflow.");
            }
        };
        // CHECK_STATUS()
        fn check_status(st: &State) {
            if st.status != CS_PARSE_OK {
                error!("Charstring encoder error: {}", st.status);
            }
        }
        let use_hintmask = (cd.flags & T1_CS_FLAG_USE_HINTMASK) != 0;
        let mut curr: usize = 0;

        self.reset_state();
        self.cs_stack_top = 0;
        // Advance Width
        if cd.sbw.wx != default_width {
            let wx = [cd.sbw.wx - nominal_width];
            self.put_numbers(&wx, 1, dst, &mut dp);
            check_status(self);
        }
        // Hint Declaration
        {
            let mut num_hstems = 0;
            let mut num_vstems = 0;
            let mut reset = true;
            let mut stem = [0.0f64; 2];

            let mut i = 0usize;
            while (i as i32) < cd.num_stems && cd.stems[i].dir == HSTEM {
                num_hstems += 1;
                stem[0] = if reset {
                    cd.stems[i].pos
                } else {
                    cd.stems[i].pos - (cd.stems[i - 1].pos + cd.stems[i - 1].del)
                };
                stem[1] = cd.stems[i].del;
                self.put_numbers(&stem, 2, dst, &mut dp);
                check_status(self);
                reset = false;
                if 2 * num_hstems > CS_ARG_STACK_MAX as i32 - 3 {
                    check_buffer(dp, 1);
                    dst[dp] = (if use_hintmask { CS_HSTEMHM } else { CS_HSTEM }) as u8;
                    dp += 1;
                    reset = true;
                }
                i += 1;
            }
            if !reset {
                check_buffer(dp, 1);
                dst[dp] = (if use_hintmask { CS_HSTEMHM } else { CS_HSTEM }) as u8;
                dp += 1;
            }
            reset = true;
            if cd.num_stems - num_hstems > 0 {
                for i in num_hstems as usize..cd.num_stems as usize {
                    num_vstems += 1;
                    stem[0] = if reset {
                        cd.stems[i].pos
                    } else {
                        cd.stems[i].pos - (cd.stems[i - 1].pos + cd.stems[i - 1].del)
                    };
                    stem[1] = cd.stems[i].del;
                    self.put_numbers(&stem, 2, dst, &mut dp);
                    check_status(self);
                    reset = false;
                    if 2 * num_vstems > CS_ARG_STACK_MAX as i32 - 3 {
                        check_buffer(dp, 1);
                        dst[dp] = (if use_hintmask { CS_VSTEMHM } else { CS_VSTEM }) as u8;
                        dp += 1;
                        reset = true;
                    }
                }
                if !reset {
                    check_buffer(dp, 1);
                    if use_hintmask || (cd.flags & T1_CS_FLAG_USE_CNTRMASK) != 0 {
                        // The vstem hint operator can be ommited if hstem and
                        // vstem hints are both declared at the beginning of a
                        // charstring, and is followed directly by the hintmask
                        // or cntrmask operators.
                        // (C dereferences a NULL charpath here.)
                        let t = cd.charpath.first().map_or(0, |c| c.type_);
                        if t != CS_HINT_DECL && t != CS_CNTRMASK {
                            dst[dp] = CS_VSTEMHM as u8;
                            dp += 1;
                        }
                    } else {
                        dst[dp] = CS_VSTEM as u8;
                        dp += 1;
                    }
                }
            }
        }
        // Path Construction and Hint Replacement
        let nmask = ((cd.num_stems + 7) / 8) as usize;
        while curr < cd.charpath.len() && cd.charpath[curr].type_ != CS_ENDCHAR {
            match cd.charpath[curr].type_ {
                CS_HINT_DECL => {
                    let mut hintmask = [0u8; (CS_STEM_ZONE_MAX + 7) / 8];
                    while curr < cd.charpath.len() && cd.charpath[curr].type_ == CS_HINT_DECL {
                        let stem_idx = get_stem(cd, cd.charpath[curr].args[0] as i32);
                        assert!(stem_idx < cd.num_stems);
                        hintmask[(stem_idx / 8) as usize] |= (1i32 << (7 - (stem_idx % 8))) as u8;
                        curr += 1;
                    }
                    if use_hintmask {
                        check_buffer(dp, nmask + 1);
                        dst[dp] = CS_HINTMASK as u8;
                        dp += 1;
                        dst[dp..dp + nmask].copy_from_slice(&hintmask[..nmask]);
                        dp += nmask;
                    }
                }
                CS_CNTRMASK => {
                    let mut cntrmask = [0u8; (CS_STEM_ZONE_MAX + 7) / 8];
                    let c = &cd.charpath[curr];
                    for i in 0..c.num_args.max(0) as usize {
                        let stem_idx = get_stem(cd, c.args[i] as i32);
                        assert!(stem_idx < cd.num_stems);
                        cntrmask[(stem_idx / 8) as usize] |= (1i32 << (7 - (stem_idx % 8))) as u8;
                    }
                    check_buffer(dp, nmask + 1);
                    dst[dp] = CS_CNTRMASK as u8;
                    dp += 1;
                    dst[dp..dp + nmask].copy_from_slice(&cntrmask[..nmask]);
                    dp += nmask;
                    curr += 1;
                }
                CS_RMOVETO | CS_HMOVETO | CS_VMOVETO | CS_RLINETO | CS_HLINETO | CS_VLINETO
                | CS_RRCURVETO | CS_HVCURVETO | CS_VHCURVETO | CS_RLINECURVE | CS_RCURVELINE => {
                    let c = &cd.charpath[curr];
                    self.put_numbers(&c.args, c.num_args, dst, &mut dp);
                    check_status(self);
                    check_buffer(dp, 1);
                    dst[dp] = c.type_ as u8;
                    dp += 1;
                    curr += 1;
                }
                CS_FLEX | CS_HFLEX | CS_HFLEX1 => {
                    let c = &cd.charpath[curr];
                    self.put_numbers(&c.args, c.num_args, dst, &mut dp);
                    check_status(self);
                    check_buffer(dp, 2);
                    dst[dp] = CS_ESCAPE as u8;
                    dst[dp + 1] = c.type_ as u8;
                    dp += 2;
                    curr += 1;
                }
                t => {
                    error!("Unknown Type 2 charstring command: {}", t);
                }
            }
        }

        // (adx ady bchar achar) endchar
        if (cd.flags & T1_CS_FLAG_USE_SEAC) != 0 {
            let seac = [
                cd.seac.adx,
                cd.seac.ady,
                f64::from(cd.seac.bchar),
                f64::from(cd.seac.achar),
            ];
            self.put_numbers(&seac, 4, dst, &mut dp);
            check_status(self);
            check_buffer(dp, 2);
            warn!(
                "Obsolete four arguments of \"endchar\" will be used for Type 1 \"seac\" operator."
            );
        }
        check_buffer(dp, 1);
        dst[dp] = CS_ENDCHAR as u8;
        dp += 1;

        dp as i32
    }
}

/// The glyph's metrics into `ginfo` (`t1char_get_metrics`' and
/// `t1char_convert_charstring`'s ends).
fn set_ginfo(cd: &T1Chardesc, ginfo: Option<&mut T1Ginfo>) {
    if let Some(ginfo) = ginfo {
        ginfo.wx = cd.sbw.wx;
        ginfo.wy = cd.sbw.wy;
        ginfo.bbox.llx = cd.bbox.llx;
        ginfo.bbox.lly = cd.bbox.lly;
        ginfo.bbox.urx = cd.bbox.urx;
        ginfo.bbox.ury = cd.bbox.ury;
        if (cd.flags & T1_CS_FLAG_USE_SEAC) != 0 {
            ginfo.use_seac = 1;
            ginfo.seac.adx = cd.seac.adx;
            ginfo.seac.ady = cd.seac.ady;
            ginfo.seac.bchar = cd.seac.bchar;
            ginfo.seac.achar = cd.seac.achar;
        } else {
            ginfo.use_seac = 0;
        }
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
        let st = &mut self.t1_char;
        let mut t1char = T1Chardesc::default();
        let cd = &mut t1char;

        init_charpath(cd);
        st.reset_state();
        st.cs_stack_top = 0;
        let mut p = 0;
        st.t1char_build_charpath(cd, src, &mut p, subrs);
        if st.cs_stack_top != 0 || st.ps_stack_top != 0 {
            warn!(
                "Stack not empty. ({}, {})",
                st.cs_stack_top, st.ps_stack_top
            );
        }
        st.do_postproc(cd);
        set_ginfo(cd, ginfo);
        release_charpath(cd);

        0
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
        let st = &mut self.t1_char;
        let mut t1char = T1Chardesc::default();
        let cd = &mut t1char;

        init_charpath(cd);
        st.reset_state();
        st.cs_stack_top = 0;
        let mut p = 0;
        st.t1char_build_charpath(cd, src, &mut p, subrs);
        if st.cs_stack_top != 0 || st.ps_stack_top != 0 {
            warn!(
                "Stack not empty. ({}, {})",
                st.cs_stack_top, st.ps_stack_top
            );
        }
        st.do_postproc(cd);
        sort_stems(cd);

        let length = st.t1char_encode_charpath(cd, default_width, nominal_width, dst);

        set_ginfo(cd, ginfo);
        release_charpath(cd);

        length
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(src: &[u8], dw: f64, nw: f64) -> (Vec<u8>, State, T1Chardesc) {
        let mut st = State::default();
        let mut cd = T1Chardesc::default();
        init_charpath(&mut cd);
        st.reset_state();
        st.cs_stack_top = 0;
        let mut p = 0;
        st.t1char_build_charpath(&mut cd, src, &mut p, None);
        st.do_postproc(&mut cd);
        sort_stems(&mut cd);
        let mut dst = [0u8; 256];
        let n = st.t1char_encode_charpath(&mut cd, dw, nw, &mut dst);
        (dst[..n as usize].to_vec(), st, cd)
    }

    #[test]
    fn simple_path() {
        // 0 500 hsbw 100 0 rmoveto 50 hlineto 50 vlineto endchar
        let src = [139, 248, 136, 13, 239, 139, 21, 189, 6, 189, 7, 14];
        let (out, _, cd) = convert(&src, 500.0, 0.0);
        assert_eq!(out, [239, 139, 21, 189, 189, 6, 14]);
        assert_eq!(
            (cd.bbox.llx, cd.bbox.lly, cd.bbox.urx, cd.bbox.ury),
            (100.0, 0.0, 150.0, 50.0)
        );
        let (out, _, _) = convert(&src, 0.0, 0.0);
        assert_eq!(out, [248, 136, 239, 139, 21, 189, 189, 6, 14]);
    }

    #[test]
    fn sidebearing_and_stems() {
        // 20 500 hsbw 10 40 hstem 5 30 vstem 30 hmoveto endchar
        let src = [159, 248, 136, 13, 149, 179, 1, 144, 169, 3, 169, 22, 14];
        let (out, _, cd) = convert(&src, 500.0, 0.0);
        assert_eq!(cd.num_stems, 2);
        // hstem: 10 40 (sby = 0); vstem: 25 30 (sbx = 20); hmoveto 50
        assert_eq!(out, [149, 179, 1, 164, 169, 3, 189, 22, 14]);
    }

    #[test]
    fn put_numbers_fixed() {
        let mut st = State::default();
        let mut dst = [0u8; 16];
        let mut dp = 0;
        st.put_numbers(&[0.5, -1131.0, 2000.0], 3, &mut dst, &mut dp);
        assert_eq!(&dst[..dp], [255, 0, 0, 0x80, 0, 0xfe, 0xff, 28, 0x07, 0xd0]);
    }
}
