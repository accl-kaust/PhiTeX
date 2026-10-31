//! Type 1 fonts as the Type1C (CFF) subsets pdfwrite embeds:
//! gdevpsf2.c's `psf_write_type2_font` (the subset's glyph order, string
//! table, Top and Private dicts, Encoding, charset, the offsets iterated
//! to convergence) and gdevpsfx.c's `psf_convert_type1_to_type2` (Type 1
//! charstrings to Type 2, Subrs expanded, hints gathered, operators
//! combined), in gs's fixed point (24.8).

use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

use crate::font::Font;
use crate::interp::Ps;
use crate::obj::{PDict, V};

type Fixed = i32;

fn int2fixed(i: i32) -> Fixed {
    i << 8
}

// ------ the glyphs' order ------

/// The sort key of a glyph name: its index in Ghostscript's name table
/// (`gs_glyph`), from the measured order; names gs had not made when the
/// EPS started after them, in the order this run made them.
pub fn glyph_key(ps: &mut Ps<'_>, name: &[u8]) -> u64 {
    let e2p = ps.dev.s.prepress;
    if ps.name_ranks.is_none() {
        let t: &[&str] = if e2p {
            &crate::gsnames::E2P
        } else {
            &crate::gsnames::DPX
        };
        let mut m = alloc::collections::BTreeMap::new();
        for (i, n) in t.iter().enumerate() {
            m.insert(n.as_bytes().to_vec(), i as u64);
        }
        ps.name_ranks = Some(m);
    }
    if let Some(r) = ps.name_ranks.as_ref().and_then(|m| m.get(name)) {
        return *r;
    }
    let n = ps.names.intern(name);
    1_000_000 + u64::from(n)
}

// ------ Type 1 charstrings to Type 2 ------

/// A Type 1 charstring program in plain text, and the font's Subrs.
struct T1Src<'f> {
    subrs: &'f [Option<Rc<Vec<u8>>>],
}

const C_HSTEM: i32 = 1;
const C_VSTEM: i32 = 3;
const C_VMOVETO: i32 = 4;
const C_RLINETO: i32 = 5;
const C_HLINETO: i32 = 6;
const C_VLINETO: i32 = 7;
const C_RRCURVETO: i32 = 8;
const C_CLOSEPATH: i32 = 9;
const C_CALLSUBR: i32 = 10;
const C_RETURN: i32 = 11;
const C_ESCAPE: i32 = 12;
const C_HSBW: i32 = 13;
const C_ENDCHAR: i32 = 14;
const C_UNDOC15: i32 = 15;
const C2_HSTEMHM: i32 = 18;
const C2_HINTMASK: i32 = 19;
const C2_VSTEMHM: i32 = 23;
const C_RMOVETO: i32 = 21;
const C_HMOVETO: i32 = 22;
const C2_RCURVELINE: i32 = 24;
const C2_RLINECURVE: i32 = 25;
const C2_VVCURVETO: i32 = 26;
const C2_HHCURVETO: i32 = 27;
const C2_SHORTINT: i32 = 28;
const C_VHCURVETO: i32 = 30;
const C_HVCURVETO: i32 = 31;
const CX_NUM4: i32 = 255;
const CE: i32 = 32;
const CE1_DOTSECTION: i32 = 0;
const CE1_VSTEM3: i32 = 1;
const CE1_HSTEM3: i32 = 2;
const CE1_SEAC: i32 = 6;
const CE1_SBW: i32 = 7;
const CE1_DIV: i32 = 12;
const CE1_UNDOC15: i32 = 15;
const CE1_CALLOTHERSUBR: i32 = 16;
const CE1_POP: i32 = 17;
const CE1_SETCURRENTPOINT: i32 = 33;
const CE2_ADD: i32 = 10;
const CE2_MUL: i32 = 24;
const CE2_FLEX: i32 = 35;
const OSTACK: usize = 48;
const MAX_HINTS: usize = 96;
const FLEX_MAX: i32 = 8;

#[derive(Clone, Copy, Default)]
struct Hint {
    v0: Fixed,
    v1: Fixed,
    index: u16,
}

#[derive(Default)]
struct Hints {
    data: Vec<Hint>,
}

/// The interpreter state of gdevpsfx.c (a `gs_type1_state`'s part).
struct St<'a> {
    ips: Vec<(Rc<Vec<u8>>, usize)>,
    os: Vec<Fixed>,
    lsb: (Fixed, Fixed),
    width: (Fixed, Fixed),
    sb_set: bool,
    width_set: bool,
    ignore_pops: i32,
    flex_count: i32,
    src: &'a T1Src<'a>,
}

enum Next {
    Op(i32),
    /// The program ran off its end (an implicit endchar).
    End,
    Err,
}

impl St<'_> {
    fn new<'a>(src: &'a T1Src<'a>, prog: Rc<Vec<u8>>) -> St<'a> {
        St {
            ips: vec![(prog, 0)],
            os: Vec::new(),
            lsb: (0, 0),
            width: (0, 0),
            sb_set: false,
            width_set: false,
            ignore_pops: 0,
            flex_count: FLEX_MAX,
            src,
        }
    }
    fn sbw(&mut self, lx: Fixed, ly: Fixed, wx: Fixed, wy: Fixed) {
        if !self.sb_set {
            self.lsb = (lx, ly);
            self.sb_set = true;
        }
        if !self.width_set {
            self.width = (wx, wy);
            self.width_set = true;
        }
    }
    fn byte(&mut self) -> Option<i32> {
        let (p, i) = self.ips.last_mut()?;
        let c = *p.get(*i)?;
        *i += 1;
        Some(i32::from(c))
    }
    /// `type1_next`.
    fn next(&mut self) -> Next {
        loop {
            let Some(c) = self.byte() else {
                return Next::End;
            };
            if c >= 32 {
                let v = if c <= 246 {
                    int2fixed(c - 139)
                } else if c <= 250 {
                    let Some(c2) = self.byte() else {
                        return Next::End;
                    };
                    int2fixed((c - 247) * 256 + c2 + 108)
                } else if c <= 254 {
                    let Some(c2) = self.byte() else {
                        return Next::End;
                    };
                    int2fixed(-(c - 251) * 256 - c2 - 108)
                } else {
                    let mut lw: i32 = 0;
                    for _ in 0..4 {
                        let Some(b) = self.byte() else {
                            return Next::End;
                        };
                        lw = (lw << 8) | b;
                    }
                    int2fixed(lw)
                };
                if self.os.len() >= OSTACK {
                    return Next::Err;
                }
                self.os.push(v);
                continue;
            }
            match c {
                0 | 2 | 17 => return Next::Err,
                C_CALLSUBR => {
                    let Some(n) = self.os.pop() else {
                        return Next::Err;
                    };
                    let k = n >> 8;
                    let Some(Some(s)) = self.src.subrs.get(k as usize).cloned() else {
                        return Next::Err;
                    };
                    if self.ips.len() > 10 {
                        return Next::Err;
                    }
                    self.ips.push((s, 0));
                    continue;
                }
                C_RETURN => {
                    if self.ips.len() > 1 {
                        self.ips.pop();
                        continue;
                    }
                    return Next::Err;
                }
                C_UNDOC15 => {
                    self.os.clear();
                    continue;
                }
                C_ESCAPE => {
                    let Some(e) = self.byte() else {
                        return Next::End;
                    };
                    match e {
                        CE1_DIV => {
                            if self.os.len() < 2 {
                                return Next::Err;
                            }
                            let b = self.os.pop().unwrap_or(1);
                            let a = self.os.pop().unwrap_or(0);
                            let q = f64::from(a) / f64::from(b);
                            self.os.push((q * 256.0) as Fixed);
                            continue;
                        }
                        CE1_UNDOC15 => {
                            self.os.clear();
                            continue;
                        }
                        CE1_CALLOTHERSUBR => {
                            if self.os.len() < 2 {
                                return Next::Err;
                            }
                            match self.os[self.os.len() - 1] >> 8 {
                                0 => self.ignore_pops = 2,
                                3 => self.ignore_pops = 1,
                                14..=18 => return Next::Err,
                                _ => {}
                            }
                            return Next::Op(CE + e);
                        }
                        CE1_POP => {
                            if self.ignore_pops != 0 {
                                self.ignore_pops -= 1;
                                continue;
                            }
                            return Next::Err;
                        }
                        _ => return Next::Op(CE + e),
                    }
                }
                _ => return Next::Op(c),
            }
        }
    }
}

/// `type1_stem1`: adds a hint, sorted; a duplicate sets its bit in
/// `active`.
fn stem1(t: &mut Hints, v: Fixed, dv: Fixed, lsb: Fixed, active: Option<&mut [u8]>) -> bool {
    let v0 = v + lsb;
    let v1 = v0 + dv;
    if t.data.len() >= MAX_HINTS {
        return false;
    }
    let mut top = t.data.len();
    while top > 0
        && (v0 < t.data[top - 1].v0 || (v0 == t.data[top - 1].v0 && v1 < t.data[top - 1].v1))
    {
        top -= 1;
    }
    if top > 0 && v0 == t.data[top - 1].v0 && v1 == t.data[top - 1].v1 {
        if let Some(a) = active {
            let index = usize::from(t.data[top - 1].index);
            a[index >> 3] |= 0x80 >> (index & 7);
        }
        return true;
    }
    t.data.insert(top, Hint { v0, v1, index: 0 });
    true
}

fn put_op(s: &mut Vec<u8>, op: i32) {
    if op >= CE {
        s.push(C_ESCAPE as u8);
        s.push((op - CE) as u8);
    } else {
        s.push(op as u8);
    }
}

fn put_int(s: &mut Vec<u8>, i: i32) {
    if (-107..=107).contains(&i) {
        s.push((i + 139) as u8);
    } else if (0..=1131).contains(&i) {
        let v = (247 << 8) + i - 108;
        s.push((v >> 8) as u8);
        s.push(v as u8);
    } else if (-1131..0).contains(&i) {
        let v = (251 << 8) - i - 108;
        s.push((v >> 8) as u8);
        s.push(v as u8);
    } else if (-32768..=32767).contains(&i) {
        s.push(C2_SHORTINT as u8);
        s.push((i >> 8) as u8);
        s.push(i as u8);
    } else {
        put_int(s, i >> 10);
        put_int(s, 1024);
        put_op(s, CE + CE2_MUL);
        put_int(s, i & 1023);
        put_op(s, CE + CE2_ADD);
    }
}

fn put_fixed(s: &mut Vec<u8>, v: Fixed) {
    if v & 0xff == 0 {
        put_int(s, v >> 8);
    } else if v >= int2fixed(-32768) && v < int2fixed(32768) {
        s.push(CX_NUM4 as u8);
        let w = v << 8;
        s.extend_from_slice(&w.to_be_bytes());
    } else {
        put_int(s, v >> 8);
        put_fixed(s, v & 0xff);
        put_op(s, CE + CE2_ADD);
    }
}

fn put_stems(s: &mut Vec<u8>, os_count: usize, t: &Hints, op: i32) {
    let mut prev = 0;
    let mut pushed = os_count;
    for h in &t.data {
        if pushed > OSTACK - 2 {
            put_op(s, op);
            pushed = 0;
        }
        put_fixed(s, h.v0 - prev);
        put_fixed(s, h.v1 - h.v0);
        prev = h.v1;
        pushed += 2;
    }
    put_op(s, op);
}

/// `psf_convert_type1_to_type2` for one charstring.
fn convert(src: &T1Src<'_>, prog: Rc<Vec<u8>>) -> Option<Vec<u8>> {
    let mut s = Vec::new();
    let mut hstem = Hints::default();
    let mut vstem = Hints::default();
    let mut replace_hints = false;
    // (the first pass: the hints)
    {
        let mut st = St::new(src, prog.clone());
        loop {
            let c = match st.next() {
                Next::Op(c) => c,
                Next::End => break,
                Next::Err => return None,
            };
            let n = st.os.len();
            match c {
                C_HSBW => {
                    if n < 2 {
                        return None;
                    }
                    let (a, b) = (st.os[0], st.os[1]);
                    st.sbw(a, 0, b, 0);
                    st.os.clear();
                }
                C_HSTEM | C_VSTEM => {
                    if n < 2 {
                        return None;
                    }
                    let (v, dv) = (st.os[n - 2], st.os[n - 1]);
                    if c == C_HSTEM {
                        stem1(&mut hstem, v, dv, st.lsb.1, None);
                    } else {
                        stem1(&mut vstem, v, dv, st.lsb.0, None);
                    }
                    st.os.clear();
                }
                x if x == CE + CE1_SBW => {
                    if n < 4 {
                        return None;
                    }
                    let (a, b, cc, d) = (st.os[0], st.os[1], st.os[2], st.os[3]);
                    st.sbw(a, b, cc, d);
                    st.os.clear();
                }
                x if x == CE + CE1_VSTEM3 || x == CE + CE1_HSTEM3 => {
                    if n < 6 {
                        return None;
                    }
                    let v: Vec<Fixed> = st.os[n - 6..].to_vec();
                    let (t, l) = if x == CE + CE1_VSTEM3 {
                        (&mut vstem, st.lsb.0)
                    } else {
                        (&mut hstem, st.lsb.1)
                    };
                    for k in 0..3 {
                        if !stem1(t, v[2 * k], v[2 * k + 1], l, None) {
                            break;
                        }
                    }
                    st.os.clear();
                }
                x if x == CE + CE1_CALLOTHERSUBR => {
                    if n < 2 {
                        return None;
                    }
                    let top = st.os[n - 1];
                    if top == int2fixed(3) {
                        replace_hints = true;
                    }
                    let k = st.os[n - 2] >> 8;
                    if k < 0 || n < 2 + k as usize {
                        return None;
                    }
                    if top == int2fixed(12) || top == int2fixed(13) {
                        st.os.truncate(n - k as usize);
                    }
                    let m = st.os.len();
                    st.os.truncate(m - 2);
                }
                x if x == CE + CE1_DOTSECTION => replace_hints = true,
                x if x == CE + CE1_SEAC => {
                    if n < 5 {
                        return None;
                    }
                    break;
                }
                C_ENDCHAR => break,
                _ => st.os.clear(),
            }
        }
    }
    for (i, h) in hstem.data.iter_mut().enumerate() {
        h.index = i as u16;
    }
    let nh = hstem.data.len();
    for (i, h) in vstem.data.iter_mut().enumerate() {
        h.index = (i + nh) as u16;
    }
    let hintmask_size = if replace_hints {
        (nh + vstem.data.len()).div_ceil(8)
    } else {
        0
    };
    let mut active = vec![0u8; MAX_HINTS.div_ceil(8)];
    let mut dot_save = vec![0u8; MAX_HINTS.div_ceil(8)];
    // (the second pass: the result)
    let mut st = St::new(src, prog);
    let mut first = true;
    let mut need_moveto = true;
    let mut hints_changed = false;
    let mut width_on_stack = false;
    let mut dot_in = false;
    let (mut mx0, mut my0) = (0, 0);
    let mut depth: usize = 0;
    let mut prev_op: i32 = -1;
    macro_rules! check_op {
        () => {
            if prev_op >= 0 {
                put_op(&mut s, prev_op);
                depth = 0;
                prev_op = -1;
            }
        };
    }
    macro_rules! check_hints_changed {
        () => {
            if hints_changed {
                put_op(&mut s, C2_HINTMASK);
                s.extend_from_slice(&active[..hintmask_size]);
                hints_changed = false;
            }
        };
    }
    macro_rules! hints_changed_ {
        () => {
            hints_changed = replace_hints;
            if hints_changed {
                check_op!();
            }
        };
    }
    loop {
        let mut c = match st.next() {
            Next::Op(c) => c,
            Next::End => {
                put_op(&mut s, C_ENDCHAR);
                return Some(s);
            }
            Next::Err => return None,
        };
        if need_moveto
            && ((C_RLINETO..=C_RRCURVETO).contains(&c) || c == C_VHCURVETO || c == C_HVCURVETO)
        {
            let (mut mx, mut my) = (0, 0);
            need_moveto = false;
            check_op!();
            if first {
                if width_on_stack {
                    if st.os.is_empty() {
                        return None;
                    }
                    let w = st.os[st.os.len() - 1];
                    put_fixed(&mut s, w);
                    st.os.remove(0);
                }
                mx += st.lsb.0 + mx0;
                my += st.lsb.1 + my0;
                first = false;
            }
            check_hints_changed!();
            if mx == 0 {
                put_fixed(&mut s, my);
                depth = 1;
                prev_op = C_VMOVETO;
            } else if my == 0 {
                put_fixed(&mut s, mx);
                depth = 1;
                prev_op = C_HMOVETO;
            } else {
                put_fixed(&mut s, mx);
                put_fixed(&mut s, my);
                depth = 2;
                prev_op = C_RMOVETO;
            }
        }
        let n = st.os.len();
        // (what to do: copy, put, or handled)
        enum Do {
            Copy,
            Put,
            Done,
        }
        let what: Do = match c {
            C_HSTEM | C_VSTEM => {
                if n < 2 {
                    return None;
                }
                let (v, dv) = (st.os[n - 2], st.os[n - 1]);
                if c == C_HSTEM {
                    stem1(&mut hstem, v, dv, st.lsb.1, Some(&mut active));
                } else {
                    stem1(&mut vstem, v, dv, st.lsb.0, Some(&mut active));
                }
                hints_changed_!();
                st.os.clear();
                Do::Done
            }
            x if x == CE + CE1_VSTEM3 || x == CE + CE1_HSTEM3 => {
                if n < 6 {
                    return None;
                }
                let v: Vec<Fixed> = st.os[n - 6..].to_vec();
                let (t, l) = if x == CE + CE1_VSTEM3 {
                    (&mut vstem, st.lsb.0)
                } else {
                    (&mut hstem, st.lsb.1)
                };
                for k in 0..3 {
                    if !stem1(t, v[2 * k], v[2 * k + 1], l, Some(&mut active)) {
                        break;
                    }
                }
                hints_changed_!();
                st.os.clear();
                Do::Done
            }
            x if x == CE + CE1_DOTSECTION => {
                if dot_in {
                    active[..hintmask_size].copy_from_slice(&dot_save[..hintmask_size]);
                    dot_in = false;
                } else {
                    dot_save[..hintmask_size].copy_from_slice(&active[..hintmask_size]);
                    active[..hintmask_size].fill(0);
                    dot_in = true;
                }
                hints_changed_!();
                Do::Done
            }
            C_CLOSEPATH => {
                need_moveto = true;
                Do::Done
            }
            x if x == CE + CE1_SETCURRENTPOINT => {
                if first {
                    if n < 2 {
                        return None;
                    }
                    mx0 = st.os[n - 2];
                    my0 = st.os[n - 1];
                }
                Do::Done
            }
            C_VMOVETO | C_HMOVETO | C_RMOVETO => {
                let (mut mx, mut my) = match c {
                    C_VMOVETO => {
                        if n < 1 {
                            return None;
                        }
                        (0, st.os.pop().unwrap_or(0))
                    }
                    C_HMOVETO => {
                        if n < 1 {
                            return None;
                        }
                        (st.os.pop().unwrap_or(0), 0)
                    }
                    _ => {
                        if n < 2 {
                            return None;
                        }
                        let y = st.os.pop().unwrap_or(0);
                        let x = st.os.pop().unwrap_or(0);
                        (x, y)
                    }
                };
                need_moveto = false;
                check_op!();
                if first {
                    if let Some(&w) = st.os.last() {
                        put_fixed(&mut s, w);
                    }
                    mx += st.lsb.0 + mx0;
                    my += st.lsb.1 + my0;
                    first = false;
                }
                if st.flex_count != FLEX_MAX {
                    // (a flex point: kept on the stack)
                    match st.next() {
                        Next::Op(x) if x == CE + CE1_CALLOTHERSUBR => {}
                        _ => return None,
                    }
                    let m = st.os.len();
                    if m < 2 || st.os[m - 1] != int2fixed(2) || st.os[m - 2] != 0 {
                        return None;
                    }
                    st.flex_count += 1;
                    st.os[m - 2] = mx;
                    st.os[m - 1] = my;
                    continue;
                }
                check_hints_changed!();
                if mx == 0 {
                    put_fixed(&mut s, my);
                    depth = 1;
                    prev_op = C_VMOVETO;
                } else if my == 0 {
                    put_fixed(&mut s, mx);
                    depth = 1;
                    prev_op = C_HMOVETO;
                } else {
                    put_fixed(&mut s, mx);
                    put_fixed(&mut s, my);
                    depth = 2;
                    prev_op = C_RMOVETO;
                }
                st.os.clear();
                Do::Done
            }
            x if x == C_HSBW || x == CE + CE1_SBW => Do::Done,
            x if x == CE + CE1_SEAC => {
                if n < 5 {
                    return None;
                }
                let asb = st.os[n - 5];
                st.os[n - 4] += st.lsb.0 - asb;
                st.os.remove(n - 5);
                check_op!();
                for &v in &st.os {
                    put_fixed(&mut s, v);
                }
                put_op(&mut s, C_ENDCHAR);
                return Some(s);
            }
            C_ENDCHAR => {
                check_op!();
                for &v in &st.os {
                    put_fixed(&mut s, v);
                }
                put_op(&mut s, C_ENDCHAR);
                return Some(s);
            }
            x if x == CE + CE1_CALLOTHERSUBR => {
                if n < 2 {
                    return None;
                }
                check_op!();
                match st.os[n - 1] >> 8 {
                    0 => {
                        if n < 19 {
                            return None;
                        }
                        let b = n - 19;
                        st.os[b] += st.os[b + 2];
                        st.os[b + 1] += st.os[b + 3];
                        st.os.copy_within(b + 4..b + 15, b + 2);
                        st.os.truncate(n - 6);
                        c = CE + CE2_FLEX;
                        st.flex_count = FLEX_MAX;
                        st.ignore_pops = 2;
                        Do::Copy
                    }
                    1 => {
                        st.flex_count = 0;
                        st.os.truncate(n - 2);
                        Do::Done
                    }
                    3 => {
                        active[..hintmask_size].fill(0);
                        hints_changed_!();
                        st.ignore_pops = 1;
                        st.os.truncate(n - 2);
                        Do::Done
                    }
                    12 | 13 => {
                        let k = st.os[n - 2] >> 8;
                        if k < 0 || n < 2 + k as usize {
                            return None;
                        }
                        st.os.truncate(n - 2 - k as usize);
                        Do::Done
                    }
                    _ => return None,
                }
            }
            C_RLINETO => {
                if n < 2 {
                    return None;
                }
                if depth > OSTACK - 2 {
                    Do::Copy
                } else if prev_op == C_RLINETO {
                    Do::Put
                } else if prev_op == C_RRCURVETO {
                    c = C2_RCURVELINE;
                    Do::Put
                } else {
                    Do::Copy
                }
            }
            C_HLINETO | C_VLINETO => {
                if n < 1 {
                    return None;
                }
                let (odd, even) = if c == C_HLINETO {
                    (C_VLINETO, C_HLINETO)
                } else {
                    (C_HLINETO, C_VLINETO)
                };
                if depth > OSTACK - 1 || prev_op != (if depth & 1 != 0 { odd } else { even }) {
                    Do::Copy
                } else {
                    c = prev_op;
                    Do::Put
                }
            }
            C_HVCURVETO | C_VHCURVETO => {
                if n < 4 {
                    return None;
                }
                let (a, b) = if c == C_HVCURVETO {
                    (C_VHCURVETO, C_HVCURVETO)
                } else {
                    (C_HVCURVETO, C_VHCURVETO)
                };
                if depth & 1 != 0
                    || depth > OSTACK - 4
                    || prev_op != (if depth & 4 != 0 { a } else { b })
                {
                    Do::Copy
                } else {
                    c = prev_op;
                    Do::Put
                }
            }
            C_RRCURVETO => {
                if n < 6 {
                    return None;
                }
                let os = &mut st.os;
                let t = n - 1;
                if depth == 0 {
                    if os[t - 1] == 0 {
                        c = C2_VVCURVETO;
                        os[t - 1] = os[t];
                        if os[t - 5] == 0 {
                            os.copy_within(t - 4..t, t - 5);
                            os.truncate(n - 2);
                        } else {
                            os.truncate(n - 1);
                        }
                    } else if os[t] == 0 {
                        c = C2_HHCURVETO;
                        if os[t - 4] == 0 {
                            os.copy_within(t - 3..t, t - 4);
                            os.truncate(n - 2);
                        } else {
                            os[t] = os[t - 5];
                            os[t - 5] = os[t - 4];
                            os[t - 4] = os[t];
                            os.truncate(n - 1);
                        }
                    }
                    Do::Copy
                } else if depth > OSTACK - 6 {
                    Do::Copy
                } else {
                    match prev_op {
                        C2_HHCURVETO => {
                            if os[t - 4] == 0 && os[t] == 0 {
                                os.copy_within(t - 3..t, t - 4);
                                c = prev_op;
                                os.truncate(n - 2);
                                Do::Put
                            } else {
                                Do::Copy
                            }
                        }
                        C2_VVCURVETO => {
                            if os[t - 5] == 0 && os[t - 1] == 0 {
                                os.copy_within(t - 4..t - 1, t - 5);
                                os[t - 2] = os[t];
                                c = prev_op;
                                os.truncate(n - 2);
                                Do::Put
                            } else {
                                Do::Copy
                            }
                        }
                        C_HVCURVETO | C_VHCURVETO => {
                            // (vrc / hrc)
                            let vrc = if prev_op == C_HVCURVETO {
                                depth & 4 != 0
                            } else {
                                depth & 4 == 0
                            };
                            if depth & 1 != 0 {
                                Do::Copy
                            } else if vrc {
                                if os[t - 5] != 0 {
                                    Do::Copy
                                } else {
                                    os.copy_within(t - 4..=t, t - 5);
                                    c = prev_op;
                                    os.truncate(n - 1);
                                    Do::Put
                                }
                            } else if os[t - 4] != 0 {
                                Do::Copy
                            } else {
                                os.copy_within(t - 3..t - 1, t - 4);
                                os[t - 2] = os[t];
                                c = prev_op;
                                os.truncate(n - 1);
                                Do::Put
                            }
                        }
                        C_RLINETO => {
                            c = C2_RLINECURVE;
                            Do::Put
                        }
                        C_RRCURVETO => Do::Put,
                        _ => Do::Copy,
                    }
                }
            }
            x if x >= CE => return None,
            _ => Do::Copy,
        };
        // (hsbw / sbw: the width and the hints)
        if c == C_HSBW || c == CE + CE1_SBW {
            if c == C_HSBW {
                if n < 2 {
                    return None;
                }
                let (a, b) = (st.os[0], st.os[1]);
                st.sbw(a, 0, b, 0);
                st.os[0] = st.os[1];
            } else {
                if n < 4 {
                    return None;
                }
                let (a, b, cc, d) = (st.os[0], st.os[1], st.os[2], st.os[3]);
                st.sbw(a, b, cc, d);
                st.os[0] = st.os[2];
            }
            if st.os[0] == 0 {
                st.os.clear();
            } else {
                st.os.truncate(1);
                width_on_stack = true;
            }
            if !hstem.data.is_empty() {
                if let Some(&w) = st.os.first() {
                    put_fixed(&mut s, w);
                }
                put_stems(
                    &mut s,
                    st.os.len(),
                    &hstem,
                    if replace_hints { C2_HSTEMHM } else { C_HSTEM },
                );
                st.os.clear();
                width_on_stack = false;
            }
            if !vstem.data.is_empty() {
                if let Some(&w) = st.os.first() {
                    put_fixed(&mut s, w);
                }
                put_stems(
                    &mut s,
                    st.os.len(),
                    &vstem,
                    if replace_hints { C2_VSTEMHM } else { C_VSTEM },
                );
                st.os.clear();
                width_on_stack = false;
            }
            continue;
        }
        match what {
            Do::Done => {}
            Do::Copy | Do::Put => {
                if matches!(what, Do::Copy) {
                    check_op!();
                    check_hints_changed!();
                }
                for &v in &st.os {
                    put_fixed(&mut s, v);
                }
                depth += st.os.len();
                prev_op = c;
                st.os.clear();
            }
        }
    }
}

// ------ the CFF ------

/// The string table (`cff_string_table_t`): custom strings in order.
struct Strings {
    items: Vec<Vec<u8>>,
}

impl Strings {
    fn sid(&mut self, s: &[u8]) -> i32 {
        if let Some(i) = crate::enc::CFF_STANDARD_STRINGS
            .iter()
            .position(|x| x.as_bytes() == s)
        {
            return i as i32;
        }
        if let Some(i) = self.items.iter().position(|x| x == s) {
            return 391 + i as i32;
        }
        self.items.push(s.to_vec());
        390 + self.items.len() as i32
    }
    fn total(&self) -> usize {
        self.items.iter().map(Vec::len).sum()
    }
}

fn offset_size(mut o: usize) -> usize {
    let mut n = 1;
    while o > 255 {
        o >>= 8;
        n += 1;
    }
    n
}

fn put_offset(s: &mut Vec<u8>, size: usize, o: usize) {
    for i in (0..size).rev() {
        s.push((o >> (i * 8)) as u8);
    }
}

/// `cff_put_Index_header`: returns the offset size.
fn index_header(s: &mut Vec<u8>, count: usize, total: usize) -> usize {
    s.push((count >> 8) as u8);
    s.push(count as u8);
    if count > 0 {
        let os = offset_size(total + 1);
        s.push(os as u8);
        put_offset(s, os, 1);
        return os;
    }
    1
}

fn index_size(count: usize, total: usize) -> usize {
    if count == 0 {
        2
    } else {
        3 + offset_size(total + 1) * (count + 1) + total
    }
}

fn d_int(s: &mut Vec<u8>, i: i32) {
    if (-107..=107).contains(&i) {
        s.push((i + 139) as u8);
    } else if (0..=1131).contains(&i) {
        let v = (247 << 8) + i - 108;
        s.push((v >> 8) as u8);
        s.push(v as u8);
    } else if (-1131..0).contains(&i) {
        let v = (251 << 8) - i - 108;
        s.push((v >> 8) as u8);
        s.push(v as u8);
    } else if (-32768..=32767).contains(&i) {
        s.push(28);
        s.push((i >> 8) as u8);
        s.push(i as u8);
    } else {
        s.push(29);
        s.extend_from_slice(&i.to_be_bytes());
    }
}

fn d_real(s: &mut Vec<u8>, f: f64) {
    if f == f64::from(f as i32) {
        d_int(s, f as i32);
        return;
    }
    let t = crate::num::g(f);
    s.push(30);
    let mut b: u8 = 0xff;
    let tb = t.as_bytes();
    let mut i = 0;
    while i < tb.len() {
        let d = match tb[i] {
            b'.' => 0xa,
            b'+' => {
                i += 1;
                continue;
            }
            b'-' => 0xe,
            b'e' | b'E' => {
                if tb.get(i + 1) == Some(&b'-') {
                    i += 1;
                    0xc
                } else {
                    0xb
                }
            }
            c @ b'0'..=b'9' => c - b'0',
            _ => 0xd,
        };
        if b == 0xff {
            b = (d << 4) | 0xf;
        } else {
            s.push((b & 0xf0) | d);
            b = 0xff;
        }
        i += 1;
    }
    s.push(b);
}

fn d_op(s: &mut Vec<u8>, op: i32) {
    if op >= 32 {
        s.push(12);
        s.push((op - 32) as u8);
    } else {
        s.push(op as u8);
    }
}

/// The font's values the CFF takes.
struct Info {
    notice: Option<Vec<u8>>,
    full_name: Option<Vec<u8>>,
    family_name: Option<Vec<u8>>,
    copyright: Option<Vec<u8>>,
    fixed_pitch: bool,
    paint_type: i32,
    matrix: [f32; 6],
    stroke_width: f32,
    unique_id: Option<i32>,
    // Private
    blues: [Vec<f32>; 4],
    std_hw: Option<f32>,
    std_vw: Option<f32>,
    blue_scale: f32,
    blue_shift: f32,
    blue_fuzz: i32,
    snap_h: Vec<f32>,
    snap_v: Vec<f32>,
    force_bold: bool,
    language_group: i32,
    expansion: f32,
    has_subrs: bool,
}

fn write_top(s: &mut Vec<u8>, st: &mut Strings, info: &Info, bbox: &[i32; 4], offs: [usize; 5]) {
    let [enc, charset, charstrings, private, private_size] = offs;
    let mut sv = |s: &mut Vec<u8>, v: &Option<Vec<u8>>, op: i32| {
        if let Some(v) = v {
            let sid = st.sid(v);
            d_int(s, sid);
            d_op(s, op);
        }
    };
    sv(s, &info.notice, 1);
    sv(s, &info.full_name, 2);
    sv(s, &info.family_name, 3);
    if bbox.iter().any(|&v| v != 0) {
        for &v in bbox {
            d_real(s, f64::from(v));
        }
        d_op(s, 5);
    }
    if let Some(u) = info.unique_id {
        d_int(s, u);
        d_op(s, 13);
    }
    sv(s, &info.copyright, 32);
    if info.fixed_pitch {
        d_int(s, 1);
        d_op(s, 33);
    }
    // (ItalicAngle 0, UnderlinePosition 0 and UnderlineThickness 0: the
    // copied font's font_info, which gs fills only partly)
    d_int(s, 0);
    d_op(s, 35);
    d_int(s, 0);
    d_op(s, 36);
    if info.paint_type != 0 {
        d_int(s, info.paint_type);
        d_op(s, 37);
    }
    let m = info.matrix;
    if m[0] != 0.001 || m[1] != 0.0 || m[2] != 0.0 || m[3] != 0.001 || m[4] != 0.0 || m[5] != 0.0 {
        for v in m {
            d_real(s, f64::from(v));
        }
        d_op(s, 39);
    }
    if info.stroke_width != 0.0 {
        d_real(s, f64::from(info.stroke_width));
        d_op(s, 40);
    }
    d_int(s, private_size as i32);
    d_int(s, private as i32);
    d_op(s, 18);
    d_int(s, charstrings as i32);
    d_op(s, 17);
    if charset != 0 {
        d_int(s, charset as i32);
        d_op(s, 15);
    }
    if enc != 0 {
        d_int(s, enc as i32);
        d_op(s, 16);
    }
}

fn write_private(s: &mut Vec<u8>, info: &Info, subrs_offset: usize) {
    let delta = |s: &mut Vec<u8>, v: &[f32], op: i32| {
        if v.is_empty() {
            return;
        }
        let mut prev = 0.0f32;
        for &f in v {
            d_real(s, f64::from(f - prev));
            prev = f;
        }
        d_op(s, op);
    };
    delta(s, &info.blues[0], 6);
    delta(s, &info.blues[1], 7);
    delta(s, &info.blues[2], 8);
    delta(s, &info.blues[3], 9);
    if let Some(v) = info.std_hw {
        d_real(s, f64::from(v));
        d_op(s, 10);
    }
    if let Some(v) = info.std_vw {
        d_real(s, f64::from(v));
        d_op(s, 11);
    }
    if subrs_offset != 0 {
        d_int(s, subrs_offset as i32);
        d_op(s, 19);
    }
    if info.blue_scale != 0.039625f32 {
        d_real(s, f64::from(info.blue_scale));
        d_op(s, 41);
    }
    if info.blue_shift != 7.0 {
        d_real(s, f64::from(info.blue_shift));
        d_op(s, 42);
    }
    if info.blue_fuzz != 1 {
        d_int(s, info.blue_fuzz);
        d_op(s, 43);
    }
    delta(s, &info.snap_h, 44);
    delta(s, &info.snap_v, 45);
    if info.force_bold {
        d_int(s, 1);
        d_op(s, 46);
    }
    if info.language_group != 0 {
        d_int(s, info.language_group);
        d_op(s, 49);
    }
    if info.expansion != 0.06f32 {
        d_real(s, f64::from(info.expansion));
        d_op(s, 50);
    }
}

fn floats(ps: &mut Ps<'_>, d: &PDict, k: &str) -> Vec<f32> {
    match ps.dget_s(d, k) {
        Some(o) => ps
            .nums_of(&o)
            .unwrap_or_default()
            .into_iter()
            .map(|x| x as f32)
            .collect(),
        None => Vec::new(),
    }
}

fn info_string(ps: &mut Ps<'_>, fi: &Option<PDict>, k: &str) -> Option<Vec<u8>> {
    let fi = fi.as_ref()?;
    match ps.dget_s(fi, k)?.v {
        V::Str(s) => Some(s.bytes()),
        _ => None,
    }
}

fn decrypt_cs(b: &[u8], len_iv: i64) -> Vec<u8> {
    if len_iv < 0 {
        b.to_vec()
    } else {
        crate::font::decrypt(b, 4330, len_iv as usize)
    }
}

/// The CFF of `font`'s `glyphs` (`.notdef` among them), with the codes
/// `used` encoded through `enc`, its FontBBox `bbox`.
pub fn write(
    ps: &mut Ps<'_>,
    font: &Font,
    glyphs: &[Vec<u8>],
    enc: &[Option<Vec<u8>>],
    used: &[bool; 256],
    bbox: &[i32; 4],
) -> Vec<u8> {
    let d = font.dict.clone();
    let private = match ps.dget_s(&d, "Private").map(|o| o.v) {
        Some(V::Dict(p)) => Some(p),
        _ => None,
    };
    let fi = match ps.dget_s(&d, "FontInfo").map(|o| o.v) {
        Some(V::Dict(p)) => Some(p),
        _ => None,
    };
    let len_iv = private
        .as_ref()
        .and_then(|p| ps.dget_s(p, "lenIV"))
        .and_then(|o| o.num())
        .unwrap_or(4.0) as i64;
    // (the Subrs, decrypted)
    let mut subrs: Vec<Option<Rc<Vec<u8>>>> = Vec::new();
    if let Some(p) = &private
        && let Some(V::Arr(a)) = ps.dget_s(p, "Subrs").map(|o| o.v)
    {
        for o in a.to_vec() {
            subrs.push(match o.v {
                V::Str(s) => Some(Rc::new(decrypt_cs(&s.bytes(), len_iv))),
                _ => None,
            });
        }
    }
    // (the glyph order: .notdef, the encoded sorted, the rest sorted)
    let mut keyed: Vec<(u64, Vec<u8>)> = glyphs
        .iter()
        .filter(|g| g.as_slice() != b".notdef")
        .map(|g| (glyph_key(ps, g), g.clone()))
        .collect();
    keyed.sort();
    let mut encoded: Vec<(u64, Vec<u8>)> = Vec::new();
    for c in 0..256 {
        if used[c]
            && let Some(g) = &enc[c]
            && g.as_slice() != b".notdef"
            && keyed.iter().any(|(_, k)| k == g)
        {
            let k = glyph_key(ps, g);
            if !encoded.iter().any(|(_, e)| e == g) {
                encoded.push((k, g.clone()));
            }
        }
    }
    encoded.sort();
    let mut order: Vec<Vec<u8>> = vec![b".notdef".to_vec()];
    order.extend(encoded.iter().map(|(_, g)| g.clone()));
    for (_, g) in &keyed {
        if !encoded.iter().any(|(_, e)| e == g) {
            order.push(g.clone());
        }
    }
    let num_enc = encoded.len();
    // (the charstrings, converted)
    let src = T1Src { subrs: &subrs };
    let mut charstrings: Vec<Vec<u8>> = Vec::new();
    for g in &order {
        let cs = ps.charstring(&d, g).map(|b| decrypt_cs(&b, len_iv));
        let t2 = cs
            .and_then(|b| convert(&src, Rc::new(b)))
            .unwrap_or_else(|| vec![C_ENDCHAR as u8]);
        charstrings.push(t2);
    }
    // (the font's values)
    let num = |ps: &mut Ps<'_>, dd: &Option<PDict>, k: &str| {
        dd.as_ref()
            .and_then(|p| ps.dget_s(p, k))
            .and_then(|o| o.num())
    };
    let matrix = {
        let m = font.matrix;
        let o = font.orig_matrix;
        let _ = m;
        [o.xx, o.xy, o.yx, o.yy, o.tx, o.ty]
    };
    let fixed_pitch = {
        // (gs_default_font_info over the whole font)
        let mut fw: i64 = 0;
        if let Some(V::Dict(csd)) = ps.dget_s(&d, "CharStrings").map(|o| o.v) {
            let names: Vec<Vec<u8>> = csd
                .data
                .borrow()
                .kobjs
                .iter()
                .filter_map(|k| match k.v {
                    V::Name(n) => Some(ps.names.get(n).to_vec()),
                    _ => None,
                })
                .collect();
            for n in names {
                let w = ps.glyph_width(&d, &n);
                if w.1 != 0.0 {
                    fw = i64::MIN;
                    break;
                } else if fw == 0 {
                    fw = w.0 as i64;
                } else if w.0 as i64 != fw {
                    fw = i64::MIN;
                    break;
                }
            }
        }
        fw > 0
    };
    let info = Info {
        notice: info_string(ps, &fi, "Notice"),
        full_name: info_string(ps, &fi, "FullName"),
        family_name: info_string(ps, &fi, "FamilyName"),
        copyright: info_string(ps, &fi, "Copyright"),
        fixed_pitch,
        paint_type: font.paint_type,
        matrix,
        stroke_width: font.stroke_width as f32,
        unique_id: ps.dget_s(&d, "UniqueID").and_then(|o| match o.v {
            V::Int(i) => Some(i as i32),
            _ => None,
        }),
        blues: [
            private
                .as_ref()
                .map(|p| floats(ps, p, "BlueValues"))
                .unwrap_or_default(),
            private
                .as_ref()
                .map(|p| floats(ps, p, "OtherBlues"))
                .unwrap_or_default(),
            private
                .as_ref()
                .map(|p| floats(ps, p, "FamilyBlues"))
                .unwrap_or_default(),
            private
                .as_ref()
                .map(|p| floats(ps, p, "FamilyOtherBlues"))
                .unwrap_or_default(),
        ],
        std_hw: private
            .as_ref()
            .map(|p| floats(ps, p, "StdHW"))
            .and_then(|v| v.first().copied()),
        std_vw: private
            .as_ref()
            .map(|p| floats(ps, p, "StdVW"))
            .and_then(|v| v.first().copied()),
        blue_scale: num(ps, &private, "BlueScale").map_or(0.039625, |x| x as f32),
        blue_shift: num(ps, &private, "BlueShift").map_or(7.0, |x| x as f32),
        blue_fuzz: num(ps, &private, "BlueFuzz").map_or(1, |x| x as i32),
        snap_h: private
            .as_ref()
            .map(|p| floats(ps, p, "StemSnapH"))
            .unwrap_or_default(),
        snap_v: private
            .as_ref()
            .map(|p| floats(ps, p, "StemSnapV"))
            .unwrap_or_default(),
        force_bold: private
            .as_ref()
            .and_then(|p| ps.dget_s(p, "ForceBold"))
            .is_some_and(|o| matches!(o.v, V::Bool(true))),
        language_group: num(ps, &private, "LanguageGroup").map_or(0, |x| x as i32),
        expansion: num(ps, &private, "ExpansionFactor").map_or(0.06, |x| x as f32),
        // (the copied font gs writes from has no Subrs: no Subrs entry)
        has_subrs: false,
    };
    // (the strings: the Top dict's, then the glyph names)
    let mut strings = Strings { items: Vec::new() };
    {
        let mut scratch = Vec::new();
        write_top(&mut scratch, &mut strings, &info, bbox, [0; 5]);
    }
    let sids: Vec<i32> = order.iter().map(|g| strings.sid(g)).collect();
    let charset_size = 1 + 2 * (order.len() - 1);
    // (the Encoding: codes of the encoded glyphs, supplements for more)
    let mut index = [0u8; 255];
    let mut seen = [false; 255];
    let mut supplement: Vec<(u8, i32)> = Vec::new();
    let mut max_enc = 0usize;
    for c in 0..256 {
        let Some(g) = &enc[c] else { continue };
        if !used[c] || g.as_slice() == b".notdef" {
            continue;
        }
        let Some(i) = encoded.iter().position(|(_, e)| e == g) else {
            continue;
        };
        if i >= 255 || seen[i] {
            let sid = strings.sid(g);
            supplement.push((c as u8, sid));
        } else {
            index[i] = c as u8;
            seen[i] = true;
            max_enc = max_enc.max(i);
        }
    }
    let _ = num_enc;
    let encoding_size =
        2 + (max_enc + 1) + 3 * supplement.len() + usize::from(!supplement.is_empty());
    let cs_total: usize = charstrings.iter().map(Vec::len).sum();
    let font_name = font.name.clone();
    // (the offsets, iterated to convergence)
    let (mut top_size, mut private_size, mut end_offset) =
        (0x7f_ffffusize, 0x7f_ffffusize, 0x7f_ffffusize);
    let subrs_size = usize::from(info.has_subrs);
    loop {
        let gsubrs_offset = 4
            + index_size(1, font_name.len())
            + index_size(1, top_size)
            + index_size(strings.items.len(), strings.total());
        let encoding_offset = gsubrs_offset + index_size(0, 0);
        let charset_offset = encoding_offset + encoding_size;
        let mut charstrings_offset = charset_offset + charset_size;
        let mut private_offset = charstrings_offset + index_size(charstrings.len(), cs_total);
        let mut subrs_offset = private_size;
        let mut write = |final_: bool,
                         top_size: &mut usize,
                         private_size: &mut usize,
                         cso: &mut usize,
                         po: &mut usize,
                         so: &mut usize|
         -> Vec<u8> {
            let _ = final_;
            let mut s = Vec::new();
            s.extend_from_slice(&[1, 0, 4]);
            s.push(if end_offset > 0x7fff { 3 } else { 2 });
            let os = index_header(&mut s, 1, font_name.len());
            put_offset(&mut s, os, font_name.len() + 1);
            s.extend_from_slice(&font_name);
            let os = index_header(&mut s, 1, *top_size);
            put_offset(&mut s, os, *top_size + 1);
            let t0 = s.len();
            let mut st2 = Strings {
                items: strings.items.clone(),
            };
            write_top(
                &mut s,
                &mut st2,
                &info,
                bbox,
                [encoding_offset, charset_offset, *cso, *po, *private_size],
            );
            *top_size = s.len() - t0;
            // (the strings)
            if strings.items.is_empty() {
                s.extend_from_slice(&[0, 0]);
            } else {
                let os = index_header(&mut s, strings.items.len(), strings.total());
                let mut off = 1;
                for it in &strings.items {
                    off += it.len();
                    put_offset(&mut s, os, off);
                }
                for it in &strings.items {
                    s.extend_from_slice(it);
                }
            }
            // (no global Subrs)
            s.extend_from_slice(&[0, 0]);
            // (the Encoding)
            s.push(if supplement.is_empty() { 0 } else { 0x80 });
            s.push((max_enc + 1) as u8);
            s.extend_from_slice(&index[..=max_enc]);
            if !supplement.is_empty() {
                s.push(supplement.len() as u8);
                for &(c, sid) in &supplement {
                    s.push(c);
                    s.push((sid >> 8) as u8);
                    s.push(sid as u8);
                }
            }
            // (the charset)
            s.push(0);
            for &sid in &sids[1..] {
                s.push((sid >> 8) as u8);
                s.push(sid as u8);
            }
            *cso = s.len();
            let os = index_header(&mut s, charstrings.len(), cs_total);
            let mut off = 1;
            for c in &charstrings {
                off += c.len();
                put_offset(&mut s, os, off);
            }
            for c in &charstrings {
                s.extend_from_slice(c);
            }
            *po = s.len();
            write_private(&mut s, &info, if subrs_size == 0 { 0 } else { *so });
            *private_size = s.len() - *po;
            *so = s.len() - *po;
            s.extend_from_slice(&[0, 0]);
            s
        };
        let out = write(
            false,
            &mut top_size,
            &mut private_size,
            &mut charstrings_offset,
            &mut private_offset,
            &mut subrs_offset,
        );
        if out.len() == end_offset {
            return write(
                true,
                &mut top_size,
                &mut private_size,
                &mut charstrings_offset,
                &mut private_offset,
                &mut subrs_offset,
            );
        }
        end_offset = out.len();
    }
}
