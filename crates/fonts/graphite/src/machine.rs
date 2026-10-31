// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/direct_machine.cpp, src/inc/Machine.h,
// src/inc/opcodes.h, Machine::Code::run in src/Code.cpp),
// LGPL-2.1-or-later.

//! The stack machine that runs rule programs over the slot map.

use crate::code::{Code, op};
use crate::rule::SlotMap;
use crate::segment::{NIL, Segment, attr};

/// `Machine::status_t`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MStatus {
    Finished,
    StackUnderflow,
    StackNotEmpty,
    StackOverflow,
    SlotOffsetOutBounds,
    DiedEarly,
}

const STACK_MAX: isize = 1 << 10;
const STACK_GUARD: isize = 2;
const STACK_LEN: usize = (STACK_MAX + 2 * STACK_GUARD) as usize;

/// `vm::Machine`: its stack (kept between runs, as the C++ keeps it) and
/// status (sticky: once a run fails, the segment fails).
pub(crate) struct Machine {
    stack: [i32; STACK_LEN],
    pub status: MStatus,
}

/// The interpreter's registers.
struct Regs {
    sp: isize,
    is: u32,
    map: usize,
}

impl Machine {
    pub(crate) fn new() -> Machine {
        Machine {
            stack: [0; STACK_LEN],
            status: MStatus::Finished,
        }
    }

    #[inline]
    fn rd(&self, i: isize) -> i32 {
        if (0..STACK_LEN as isize).contains(&i) {
            self.stack[i as usize]
        } else {
            0
        }
    }

    #[inline]
    fn wr(&mut self, i: isize, v: i32) {
        if (0..STACK_LEN as isize).contains(&i) {
            self.stack[i as usize] = v;
        }
    }

    /// `Machine::Code::run(m, map)`: `map` is an index into the slot map
    /// array (`m_slot_map`), moved as the program moves it.
    pub(crate) fn run_code(
        &mut self,
        code: &Code,
        seg: &mut Segment,
        smap: &mut SlotMap,
        map: &mut usize,
    ) -> i32 {
        let k = i32::from(code.max_ref) + i32::from(smap.context());
        if smap.size() <= k as usize || smap.get(k) == NIL {
            self.status = MStatus::SlotOffsetOutBounds;
            return 1;
        }
        self.run(code, seg, smap, map)
    }

    /// `Machine::run`.
    fn run(&mut self, code: &Code, seg: &mut Segment, smap: &mut SlotMap, map: &mut usize) -> i32 {
        let sp = self.direct_run(code, seg, smap, map);
        let mut sp = sp;
        let ret = if sp == STACK_GUARD + 1 {
            let v = self.rd(sp);
            sp -= 1;
            v
        } else {
            0
        };
        // check_final_stack
        if self.status == MStatus::Finished {
            let base = STACK_GUARD;
            let limit = base + STACK_MAX;
            if sp < base {
                self.status = MStatus::StackUnderflow;
            } else if sp >= limit {
                self.status = MStatus::StackOverflow;
            } else if sp != base {
                self.status = MStatus::StackNotEmpty;
            }
        }
        ret
    }

    /// `direct_run`: the stack pointer at the end.
    #[allow(clippy::too_many_lines)]
    fn direct_run(
        &mut self,
        code: &Code,
        seg: &mut Segment,
        smap: &mut SlotMap,
        map0: &mut usize,
    ) -> isize {
        let instrs = &code.instrs;
        let data = &code.data;
        let mut ip: usize = 0;
        let mut dp: usize = 0;
        let sb = STACK_GUARD;
        let mut r = Regs {
            sp: sb,
            is: smap.slot_map[*map0],
            map: *map0,
        };
        let mapb = 1 + usize::from(smap.context());
        let dir = smap.dir();
        let mut flags: u8 = 0;
        let d = |i: usize| data.get(i).copied().unwrap_or(0);

        macro_rules! push {
            ($v:expr) => {{
                let v: i32 = $v;
                r.sp += 1;
                self.wr(r.sp, v);
            }};
        }
        macro_rules! pop {
            () => {{
                let v = self.rd(r.sp);
                r.sp -= 1;
                v
            }};
        }
        macro_rules! top {
            () => {
                self.rd(r.sp)
            };
        }
        macro_rules! set_top {
            ($v:expr) => {{
                let v: i32 = $v;
                self.wr(r.sp, v);
            }};
        }
        // slotat(x): the map entry at map + x within [smap[-1], end)
        macro_rules! slotat {
            ($x:expr) => {{
                let i = r.map as isize + ($x) as isize;
                if i >= 0 && i < 1 + smap.size() as isize {
                    smap.slot_map[i as usize]
                } else {
                    self.status = MStatus::SlotOffsetOutBounds;
                    NIL
                }
            }};
        }
        macro_rules! positioned {
            ($slat:expr) => {{
                if ($slat == attr::POS_X || $slat == attr::POS_Y) && flags & 1 == 0 {
                    let b = smap.slot_map[1];
                    let e = smap.slot_map[smap.size()];
                    let cd = seg.currdir();
                    seg.position_slots(None, b, e, cd, true);
                    flags |= 1;
                }
            }};
        }

        'run: loop {
            let o = instrs.get(ip).copied().unwrap_or(op::RET_ZERO);
            // (EXIT: push the status, stop)
            macro_rules! exit {
                ($v:expr) => {{
                    push!($v);
                    break 'run;
                }};
            }
            macro_rules! die {
                () => {{
                    r.is = seg.last;
                    self.status = MStatus::DiedEarly;
                    exit!(1);
                }};
            }
            match o {
                op::NOP => {}
                op::PUSH_BYTE => {
                    push!(i32::from(d(dp) as i8));
                    dp += 1;
                }
                op::PUSH_BYTEU => {
                    push!(i32::from(d(dp)));
                    dp += 1;
                }
                op::PUSH_SHORT => {
                    push!(i32::from(i16::from_be_bytes([d(dp), d(dp + 1)])));
                    dp += 2;
                }
                op::PUSH_SHORTU => {
                    push!(i32::from(u16::from_be_bytes([d(dp), d(dp + 1)])));
                    dp += 2;
                }
                op::PUSH_LONG => {
                    push!(i32::from_be_bytes([d(dp), d(dp + 1), d(dp + 2), d(dp + 3)]));
                    dp += 4;
                }
                op::ADD
                | op::SUB
                | op::MUL
                | op::AND
                | op::OR
                | op::EQUAL
                | op::NOT_EQ
                | op::BITOR
                | op::BITAND => {
                    let a = pop!() as u32;
                    let t = top!() as u32;
                    let v = match o {
                        op::ADD => t.wrapping_add(a),
                        op::SUB => t.wrapping_sub(a),
                        op::MUL => t.wrapping_mul(a),
                        op::AND => u32::from(t != 0 && a != 0),
                        op::OR => u32::from(t != 0 || a != 0),
                        op::EQUAL => u32::from(t == a),
                        op::NOT_EQ => u32::from(t != a),
                        op::BITOR => t | a,
                        _ => t & a,
                    };
                    set_top!(v as i32);
                }
                op::LESS | op::GTR | op::LESS_EQ | op::GTR_EQ => {
                    let a = pop!();
                    let t = top!();
                    let v = match o {
                        op::LESS => t < a,
                        op::GTR => t > a,
                        op::LESS_EQ => t <= a,
                        _ => t >= a,
                    };
                    set_top!(i32::from(v));
                }
                op::DIV => {
                    let b = pop!();
                    let a = top!();
                    if b == 0 || (a == i32::MIN && b == -1) {
                        die!();
                    }
                    set_top!(a / b);
                }
                op::MIN => {
                    let a = pop!();
                    let b = top!();
                    if a < b {
                        set_top!(a);
                    }
                }
                op::MAX => {
                    let a = pop!();
                    let b = top!();
                    if a > b {
                        set_top!(a);
                    }
                }
                op::NEG => set_top!(top!().wrapping_neg()),
                op::TRUNC8 => set_top!(i32::from(top!() as u8)),
                op::TRUNC16 => set_top!(i32::from(top!() as u16)),
                op::COND => {
                    let f = pop!();
                    let t = pop!();
                    let c = pop!();
                    push!(if c != 0 { t } else { f });
                }
                op::NOT => set_top!(i32::from(top!() == 0)),
                op::BITNOT => set_top!(!top!()),
                op::NEXT | op::COPY_NEXT => {
                    if r.map as isize > smap.size() as isize {
                        die!();
                    }
                    if r.is != NIL {
                        if r.is == smap.highwater() {
                            smap.set_highpassed(true);
                        }
                        r.is = seg.s(r.is).next;
                    }
                    r.map += 1;
                }
                op::PUT_GLYPH_8BIT_OBS => {
                    let oc = u16::from(d(dp));
                    dp += 1;
                    let g = seg.silf.get_class_glyph(oc, 0);
                    seg.set_glyph(r.is, g);
                }
                op::PUT_SUBS_8BIT_OBS => {
                    let slot_ref = i32::from(d(dp) as i8);
                    let ic = u16::from(d(dp + 1));
                    let oc = u16::from(d(dp + 2));
                    dp += 3;
                    let slot = slotat!(slot_ref);
                    if slot != NIL {
                        let index = seg.silf.find_class_index(ic, seg.s(slot).glyphid);
                        let g = seg.silf.get_class_glyph(oc, u32::from(index));
                        seg.set_glyph(r.is, g);
                    }
                }
                op::PUT_COPY => {
                    let slot_ref = i32::from(d(dp) as i8);
                    dp += 1;
                    if r.is != NIL && !seg.s(r.is).is_deleted() {
                        let rf = slotat!(slot_ref);
                        if rf != NIL && rf != r.is {
                            let is = r.is;
                            if seg.s(is).parent != NIL || seg.s(is).child != NIL {
                                die!();
                            }
                            let prev = seg.s(is).prev;
                            let next = seg.s(is).next;
                            let user = seg.s(is).user;
                            seg.copy_user(is, rf);
                            *seg.sm(is) = *seg.s(rf);
                            let s = seg.sm(is);
                            s.child = NIL;
                            s.sibling = NIL;
                            s.user = user;
                            s.next = next;
                            s.prev = prev;
                            let p = seg.s(is).parent;
                            if p != NIL {
                                seg.child(p, is);
                            }
                        }
                        let s = seg.sm(r.is);
                        s.mark_copied(false);
                        s.mark_deleted(false);
                    }
                }
                op::INSERT => {
                    if smap.dec_max() <= 0 {
                        die!();
                    }
                    let ns = seg.new_slot();
                    if ns == NIL {
                        die!();
                    }
                    let mut iss = r.is;
                    while iss != NIL && seg.s(iss).is_deleted() {
                        iss = seg.s(iss).next;
                    }
                    if iss == NIL {
                        if seg.last != NIL {
                            let l = seg.last;
                            seg.sm(l).next = ns;
                            seg.sm(ns).prev = l;
                            seg.sm(ns).before = seg.s(l).before;
                            seg.last = ns;
                        } else {
                            seg.first = ns;
                            seg.last = ns;
                        }
                    } else if seg.s(iss).prev != NIL {
                        let p = seg.s(iss).prev;
                        seg.sm(p).next = ns;
                        seg.sm(ns).prev = p;
                        seg.sm(ns).before = seg.s(p).after;
                    } else {
                        seg.sm(ns).prev = NIL;
                        seg.sm(ns).before = seg.s(iss).before;
                        seg.first = ns;
                    }
                    seg.sm(ns).next = iss;
                    if iss != NIL {
                        seg.sm(iss).prev = ns;
                        seg.sm(ns).original = seg.s(iss).original;
                        seg.sm(ns).after = seg.s(iss).before;
                    } else if seg.s(ns).prev != NIL {
                        let p = seg.s(ns).prev;
                        seg.sm(ns).original = seg.s(p).original;
                        seg.sm(ns).after = seg.s(p).after;
                    } else {
                        seg.sm(ns).original = seg.default_original() as u32;
                    }
                    if r.is == smap.highwater() {
                        smap.set_highpassed(false);
                    }
                    r.is = ns;
                    seg.num_glyphs = seg.num_glyphs.wrapping_add(1);
                    if r.map != 0 {
                        r.map -= 1;
                    }
                }
                op::DELETE => {
                    if r.is == NIL || seg.s(r.is).is_deleted() {
                        die!();
                    }
                    let is = r.is;
                    seg.sm(is).mark_deleted(true);
                    let (p, n) = (seg.s(is).prev, seg.s(is).next);
                    if p != NIL {
                        seg.sm(p).next = n;
                    } else {
                        seg.first = n;
                    }
                    if n != NIL {
                        seg.sm(n).prev = p;
                    } else {
                        seg.last = p;
                    }
                    if is == smap.highwater() {
                        smap.set_highwater(n);
                    }
                    if p != NIL {
                        r.is = p;
                    }
                    seg.num_glyphs = seg.num_glyphs.wrapping_sub(1);
                }
                op::ASSOC => {
                    let num = usize::from(d(dp));
                    let assocs = dp + 1;
                    dp += 1 + num;
                    let (mut max, mut min) = (-1i32, -1i32);
                    for k in 0..num {
                        let sr = i32::from(d(assocs + k) as i8);
                        let ts = slotat!(sr);
                        if ts != NIL && (min == -1 || seg.s(ts).before < min) {
                            min = seg.s(ts).before;
                        }
                        if ts != NIL && seg.s(ts).after > max {
                            max = seg.s(ts).after;
                        }
                    }
                    if min > -1 {
                        let s = seg.sm(r.is);
                        s.before = min;
                        s.after = max;
                    }
                }
                op::CNTXT_ITEM => {
                    let is_arg = isize::from(d(dp) as i8);
                    let iskip = usize::from(d(dp + 1));
                    let dskip = usize::from(d(dp + 2));
                    dp += 3;
                    if mapb as isize + is_arg != r.map as isize {
                        ip += iskip;
                        dp += dskip;
                        push!(1);
                    }
                }
                op::ATTR_SET => {
                    let slat = d(dp);
                    dp += 1;
                    let val = pop!();
                    seg.set_attr(r.is, slat, 0, val as i16, smap);
                }
                op::ATTR_ADD | op::ATTR_SUB => {
                    let slat = d(dp);
                    dp += 1;
                    let val = pop!() as u32;
                    positioned!(slat);
                    let res = seg.get_attr(r.is, slat, 0) as u32;
                    let v = if o == op::ATTR_ADD {
                        val.wrapping_add(res)
                    } else {
                        res.wrapping_sub(val)
                    };
                    seg.set_attr(r.is, slat, 0, v as i16, smap);
                }
                op::ATTR_SET_SLOT => {
                    let slat = d(dp);
                    dp += 1;
                    let offset = (r.map as i32 - 1) * i32::from(slat == attr::ATT_TO);
                    let val = pop!().wrapping_add(offset);
                    seg.set_attr(r.is, slat, offset as u8, val as i16, smap);
                }
                op::IATTR_SET_SLOT => {
                    let slat = d(dp);
                    let idx = d(dp + 1);
                    dp += 2;
                    let val =
                        pop!().wrapping_add((r.map as i32 - 1) * i32::from(slat == attr::ATT_TO));
                    seg.set_attr(r.is, slat, idx, val as i16, smap);
                }
                op::PUSH_SLOT_ATTR => {
                    let slat = d(dp);
                    let slot_ref = i32::from(d(dp + 1) as i8);
                    dp += 2;
                    positioned!(slat);
                    let slot = slotat!(slot_ref);
                    if slot != NIL {
                        push!(seg.get_attr(slot, slat, 0));
                    }
                }
                op::PUSH_GLYPH_ATTR_OBS | op::PUSH_ATT_TO_GATTR_OBS => {
                    let ga = u16::from(d(dp));
                    let slot_ref = i32::from(d(dp + 1) as i8);
                    dp += 2;
                    let mut slot = slotat!(slot_ref);
                    if slot != NIL {
                        if o == op::PUSH_ATT_TO_GATTR_OBS {
                            let att = seg.s(slot).parent;
                            if att != NIL {
                                slot = att;
                            }
                        }
                        push!(i32::from(seg.glyph_attr(seg.s(slot).glyphid, ga)));
                    }
                }
                op::PUSH_GLYPH_METRIC | op::PUSH_ATT_TO_GLYPH_METRIC => {
                    let ga = d(dp);
                    let slot_ref = i32::from(d(dp + 1) as i8);
                    let level = d(dp + 2);
                    dp += 3;
                    let mut slot = slotat!(slot_ref);
                    if slot != NIL {
                        if o == op::PUSH_ATT_TO_GLYPH_METRIC {
                            let att = seg.s(slot).parent;
                            if att != NIL {
                                slot = att;
                            }
                        }
                        push!(seg.glyph_metric(slot, ga, level, dir != 0));
                    }
                }
                op::PUSH_FEAT => {
                    let feat = d(dp);
                    let slot_ref = i32::from(d(dp + 1) as i8);
                    dp += 2;
                    let slot = slotat!(slot_ref);
                    if slot != NIL {
                        let fid = seg.charinfo[seg.s(slot).original as usize].feature_id;
                        push!(seg.get_feature(usize::from(fid), feat) as i32);
                    }
                }
                op::PUSH_ISLOT_ATTR => {
                    let slat = d(dp);
                    let slot_ref = i32::from(d(dp + 1) as i8);
                    let idx = d(dp + 2);
                    dp += 3;
                    positioned!(slat);
                    let slot = slotat!(slot_ref);
                    if slot != NIL {
                        push!(seg.get_attr(slot, slat, idx));
                    }
                }
                op::POP_RET => {
                    let ret = pop!();
                    exit!(ret);
                }
                op::RET_ZERO => exit!(0),
                op::RET_TRUE => exit!(1),
                op::IATTR_SET => {
                    let slat = d(dp);
                    let idx = d(dp + 1);
                    dp += 2;
                    let val = pop!();
                    seg.set_attr(r.is, slat, idx, val as i16, smap);
                }
                op::IATTR_ADD | op::IATTR_SUB => {
                    let slat = d(dp);
                    let idx = d(dp + 1);
                    dp += 2;
                    let val = pop!() as u32;
                    positioned!(slat);
                    let res = seg.get_attr(r.is, slat, idx) as u32;
                    let v = if o == op::IATTR_ADD {
                        val.wrapping_add(res)
                    } else {
                        res.wrapping_sub(val)
                    };
                    seg.set_attr(r.is, slat, idx, v as i16, smap);
                }
                op::PUSH_PROC_STATE => {
                    dp += 1;
                    push!(1);
                }
                op::PUSH_VERSION => push!(0x0003_0000),
                op::PUT_SUBS => {
                    let slot_ref = i32::from(d(dp) as i8);
                    let ic = u16::from_be_bytes([d(dp + 1), d(dp + 2)]);
                    let oc = u16::from_be_bytes([d(dp + 3), d(dp + 4)]);
                    dp += 5;
                    let slot = slotat!(slot_ref);
                    if slot != NIL {
                        let index = seg.silf.find_class_index(ic, seg.s(slot).glyphid);
                        let g = seg.silf.get_class_glyph(oc, u32::from(index));
                        seg.set_glyph(r.is, g);
                    }
                }
                op::PUT_GLYPH => {
                    let oc = u16::from_be_bytes([d(dp), d(dp + 1)]);
                    dp += 2;
                    let g = seg.silf.get_class_glyph(oc, 0);
                    seg.set_glyph(r.is, g);
                }
                op::PUSH_GLYPH_ATTR | op::PUSH_ATT_TO_GLYPH_ATTR => {
                    let ga = u16::from_be_bytes([d(dp), d(dp + 1)]);
                    let slot_ref = i32::from(d(dp + 2) as i8);
                    dp += 3;
                    let mut slot = slotat!(slot_ref);
                    if slot != NIL {
                        if o == op::PUSH_ATT_TO_GLYPH_ATTR {
                            let att = seg.s(slot).parent;
                            if att != NIL {
                                slot = att;
                            }
                        }
                        push!(i32::from(seg.glyph_attr(seg.s(slot).glyphid, ga)));
                    }
                }
                op::TEMP_COPY => {
                    let ns = seg.new_slot();
                    if ns == NIL || r.is == NIL {
                        die!();
                    }
                    let user = seg.s(ns).user;
                    *seg.sm(ns) = *seg.s(r.is);
                    seg.sm(ns).user = user;
                    seg.copy_user(ns, r.is);
                    seg.sm(ns).mark_copied(true);
                    smap.slot_map[r.map] = ns;
                }
                op::BITSET => {
                    let m = u16::from_be_bytes([d(dp), d(dp + 1)]);
                    let v = u16::from_be_bytes([d(dp + 2), d(dp + 3)]);
                    dp += 4;
                    set_top!((top!() & !i32::from(m)) | i32::from(v));
                }
                op::SET_FEAT => {
                    let feat = d(dp);
                    let slot_ref = i32::from(d(dp + 1) as i8);
                    dp += 2;
                    let slot = slotat!(slot_ref);
                    if slot != NIL {
                        let fid = seg.charinfo[seg.s(slot).original as usize].feature_id;
                        let v = pop!();
                        seg.set_feature(usize::from(fid), feat, v as u32);
                    }
                }
                // (unimplemented opcodes never load)
                _ => exit!(0),
            }
            // ENDOP: stop where the stack is a whole order of magnitude out
            if (r.sp - sb) / STACK_MAX != 0 {
                break 'run;
            }
            ip += 1;
        }
        *map0 = r.map;
        smap.slot_map[r.map] = r.is;
        r.sp
    }
}
