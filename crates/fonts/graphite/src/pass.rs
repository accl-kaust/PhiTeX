// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Pass.cpp, src/inc/Pass.h),
// LGPL-2.1-or-later.

//! A pass: its finite state machine over glyph columns, its rules, and
//! running it over a segment (rules matched and applied left to right,
//! then collision fixing and kerning).

use alloc::vec::Vec;

use crate::be::{u8_at, u16_at, u32_at};
use crate::code::{Code, Limits, PassType};
use crate::collider::{
    COLL_END, COLL_FIX, COLL_ISCOL, COLL_ISSPACE, COLL_KERN, COLL_KNOWN, COLL_START, COLL_TEMPLOCK,
    KernCollider, ShiftCollider,
};
use crate::face::FaceLoad;
use crate::intervals::{gmax, gmin};
use crate::machine::{MStatus, Machine};
use crate::position::{Position, Rect};
use crate::rule::{MAX_RULES, MAX_SLOTS, Rule, Rules, SlotMap, rule_less};
use crate::segment::{NIL, SEG_INITCOLLISIONS, Segment};
use crate::silf::Silf;

/// `KernCollison::InWord`.
const IN_WORD: u8 = 2;

pub(crate) struct Pass {
    cols: Vec<u16>,
    rules: Vec<Rule>,
    rule_map: Vec<u16>,
    start_states: Vec<u16>,
    transitions: Vec<u16>,
    /// Each state's rules: a range of `rule_map` (capped at
    /// [`MAX_RULES`]).
    states: Vec<(u32, u32)>,
    pconstraint: Code,
    pub(crate) num_coll_runs: u8,
    kern_colls: u8,
    i_max_loop: u8,
    num_glyphs: u16,
    #[allow(dead_code)]
    num_states: u16,
    num_transition: u16,
    success_start: u16,
    num_columns: u16,
    min_pre_ctxt: u8,
    max_pre_ctxt: u8,
    col_threshold: u8,
    pub(crate) is_reverse_dir: bool,
}

#[inline]
fn sqr(x: f32) -> f32 {
    x * x
}

impl Pass {
    /// `readPass(pass_start, pass_length, subtable_base, face, pt)`: `d`
    /// is the pass, `base` its offset in the `Silf` subtable.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn read(
        d: &[u8],
        base: usize,
        face: &FaceLoad,
        silf: &Silf,
        pt: PassType,
    ) -> Option<Pass> {
        let end = d.len();
        if end < 40 {
            return None;
        }
        let flags = d[0];
        if flags & 0x1f != 0
            && (pt < PassType::Positioning
                || silf.a_collision == 0
                || !face.glyphs.has_boxes()
                || silf.flags & 0x20 == 0)
        {
            return None;
        }
        let num_coll_runs = flags & 0x7;
        let kern_colls = (flags >> 3) & 0x3;
        let is_reverse_dir = (flags >> 5) & 0x1 != 0;
        let i_max_loop = d[1].max(1);
        let num_rules = u16_at(d, 4);
        if num_rules == 0 && num_coll_runs == 0 {
            return None;
        }
        let rel = |o: u32| (o as usize).wrapping_sub(base);
        let pc_code = rel(u32_at(d, 8));
        let rc_code = rel(u32_at(d, 12));
        let a_code = rel(u32_at(d, 16));
        let num_states = u16_at(d, 24);
        let num_transition = u16_at(d, 26);
        let num_success = u16_at(d, 28);
        let num_columns = u16_at(d, 30);
        let num_ranges = usize::from(u16_at(d, 32));
        let mut p = 40usize;
        if num_transition > num_states
            || num_success > num_states
            || u32::from(num_success) + u32::from(num_transition) < u32::from(num_states)
            || (num_rules != 0 && num_ranges == 0)
            || num_columns > 0x7FFF
        {
            return None;
        }
        let success_start = num_states - num_success;
        if (p + num_ranges * 6).wrapping_sub(2) > end {
            return None;
        }
        let num_glyphs = u16_at(d, (p + num_ranges * 6).wrapping_sub(4)).wrapping_add(1);
        let ranges = p;
        p += num_ranges * 6;
        let o_rule_map = p;
        p += 2 * (usize::from(num_success) + 1);
        if o_rule_map + usize::from(num_success) * 2 > end || p > end {
            return None;
        }
        let num_entries = usize::from(u16_at(d, o_rule_map + usize::from(num_success) * 2));
        let rule_map_at = p;
        p += 2 * num_entries;
        if p + 2 > end {
            return None;
        }
        let min_pre_ctxt = u8_at(d, p);
        let max_pre_ctxt = u8_at(d, p + 1);
        p += 2;
        if min_pre_ctxt > max_pre_ctxt {
            return None;
        }
        let start_states_at = p;
        p += 2 * (usize::from(max_pre_ctxt - min_pre_ctxt) + 1);
        let sort_keys = p;
        p += 2 * usize::from(num_rules);
        let precontext = p;
        p += usize::from(num_rules);
        if p + 2 + 1 > end {
            return None;
        }
        let col_threshold = match u8_at(d, p) {
            0 => 10,
            v => v,
        };
        let pass_constraint_len = usize::from(u16_at(d, p + 1));
        p += 3;
        let o_constraint = p;
        p += 2 * (usize::from(num_rules) + 1);
        let o_actions = p;
        p += 2 * (usize::from(num_rules) + 1);
        let states_at = p;
        let tsize = usize::from(num_transition) * usize::from(num_columns);
        if 2 * tsize >= end.wrapping_sub(p) || p >= end {
            return None;
        }
        p += 2 * tsize;
        p += 1;
        if p != pc_code {
            return None;
        }
        p += pass_constraint_len;
        if p != rc_code || rc_code.wrapping_sub(pc_code) != pass_constraint_len {
            return None;
        }
        p += usize::from(u16_at(d, o_constraint + 2 * usize::from(num_rules)));
        if p != a_code {
            return None;
        }
        p += usize::from(u16_at(d, o_actions + 2 * usize::from(num_rules)));
        if p > end {
            return None;
        }
        let lim = |pre: u8, len: u16| Limits {
            pre_context: pre,
            rule_length: len,
            classes: silf.n_class,
            glyf_attrs: face.glyphs.num_attrs(),
            features: face.num_features,
            num_user: silf.a_user,
        };
        let mut pass = Pass {
            cols: Vec::new(),
            rules: Vec::new(),
            rule_map: Vec::new(),
            start_states: Vec::new(),
            transitions: Vec::new(),
            states: Vec::new(),
            pconstraint: Code::default(),
            num_coll_runs,
            kern_colls,
            i_max_loop,
            num_glyphs,
            num_states,
            num_transition,
            success_start,
            num_columns,
            min_pre_ctxt,
            max_pre_ctxt,
            col_threshold,
            is_reverse_dir,
        };
        if pass_constraint_len != 0 {
            let l = lim(u8_at(d, precontext), u16_at(d, sort_keys));
            let c = Code::load(
                true,
                d.get(pc_code..pc_code + pass_constraint_len)?,
                &l,
                PassType::Unknown,
            );
            if !c.present || c.status != crate::code::Status::Loaded {
                return None;
            }
            pass.pconstraint = c;
        }
        if num_rules != 0 {
            if !pass.read_ranges(d, ranges, num_ranges) {
                return None;
            }
            // readRules
            let nr = usize::from(num_rules);
            let ac_data = a_code;
            let rc_data = rc_code;
            let ac_data_end = ac_data + usize::from(u16_at(d, o_actions + 2 * nr));
            let rc_data_end = rc_data + usize::from(u16_at(d, o_constraint + 2 * nr));
            let mut ac_end = ac_data_end;
            let mut rc_end = rc_data_end;
            let mut rules: Vec<Option<Rule>> = (0..nr).map(|_| None).collect();
            for n in (1..=nr).rev() {
                let pre = u8_at(d, precontext + n - 1);
                let sort = u16_at(d, sort_keys + 2 * (n - 1));
                if sort > 63 || u16::from(pre) >= sort || pre > max_pre_ctxt || pre < min_pre_ctxt {
                    return None;
                }
                let ac_begin = ac_data + usize::from(u16_at(d, o_actions + 2 * (n - 1)));
                let oc = usize::from(u16_at(d, o_constraint + 2 * (n - 1)));
                let rc_begin = if oc != 0 { rc_data + oc } else { rc_end };
                if ac_begin > ac_end
                    || ac_begin > ac_data_end
                    || ac_end > ac_data_end
                    || rc_begin > rc_end
                    || rc_begin > rc_data_end
                    || rc_end > rc_data_end
                {
                    return None;
                }
                let l = lim(pre, sort);
                let action = Code::load(false, d.get(ac_begin..ac_end)?, &l, pt);
                let constraint = Code::load(true, d.get(rc_begin..rc_end)?, &l, pt);
                if action.status != crate::code::Status::Loaded
                    || constraint.status != crate::code::Status::Loaded
                    || !constraint.immutable()
                {
                    return None;
                }
                rules[n - 1] = Some(Rule {
                    constraint,
                    action,
                    sort,
                    pre_context: pre,
                });
                ac_end = ac_begin;
                rc_end = rc_begin;
            }
            pass.rules = rules.into_iter().map(|r| r.expect("rule")).collect();
            pass.rule_map.reserve(num_entries);
            for k in 0..num_entries {
                let rn = u16_at(d, rule_map_at + 2 * k);
                if rn >= num_rules {
                    return None;
                }
                pass.rule_map.push(rn);
            }
            // readStates
            let nstart = usize::from(max_pre_ctxt - min_pre_ctxt) + 1;
            for k in 0..nstart {
                let s = u16_at(d, start_states_at + 2 * k);
                if s >= num_states {
                    return None;
                }
                pass.start_states.push(s);
            }
            pass.transitions.reserve(tsize);
            for k in 0..tsize {
                let t = u16_at(d, states_at + 2 * k);
                if t >= num_states {
                    return None;
                }
                pass.transitions.push(t);
            }
            let success_begin = usize::from(num_states - num_success);
            let mut orm = o_rule_map;
            for i in 0..usize::from(num_states) {
                if i < success_begin {
                    pass.states.push((0, 0));
                    continue;
                }
                let b = usize::from(u16_at(d, orm));
                orm += 2;
                let e = usize::from(u16_at(d, orm));
                if b >= num_entries || e > num_entries || b > e {
                    return None;
                }
                let capped = if e - b <= MAX_RULES { e } else { b + MAX_RULES };
                pass.states.push((b as u32, capped as u32));
                // (qsort by rule length, then index; glibc's is stable)
                let rules = &pass.rules;
                pass.rule_map[b..e].sort_by(|&x, &y| {
                    if rule_less(rules, x, y) {
                        core::cmp::Ordering::Less
                    } else if rule_less(rules, y, x) {
                        core::cmp::Ordering::Greater
                    } else {
                        core::cmp::Ordering::Equal
                    }
                });
            }
        }
        Some(pass)
    }

    /// `readRanges`.
    fn read_ranges(&mut self, d: &[u8], ranges: usize, n: usize) -> bool {
        self.cols = alloc::vec![0xFFFF; usize::from(self.num_glyphs)];
        for k in 0..n {
            let first = usize::from(u16_at(d, ranges + 6 * k));
            let last = usize::from(u16_at(d, ranges + 6 * k + 2));
            let col = u16_at(d, ranges + 6 * k + 4);
            let (mut ci, ci_end) = (first, last + 1);
            if ci >= ci_end || ci_end > self.cols.len() || col >= self.num_columns {
                return false;
            }
            while ci != ci_end && self.cols[ci] == 0xffff {
                self.cols[ci] = col;
                ci += 1;
            }
            if ci != ci_end {
                return false;
            }
        }
        true
    }

    /// `runGraphite(m, fsm, reverse)`.
    pub(crate) fn run_graphite(
        &self,
        m: &mut Machine,
        seg: &mut Segment,
        smap: &mut SlotMap,
        reverse: bool,
    ) -> bool {
        let mut s = seg.first;
        if s == NIL || !self.test_pass_constraint(m, seg, smap) {
            return true;
        }
        if reverse {
            seg.reverse_slots();
            s = seg.first;
        }
        if !self.rules.is_empty() {
            let mut rules = Rules::new();
            let curr_high = seg.s(s).next;
            smap.set_highwater(curr_high);
            let mut lc = i32::from(self.i_max_loop);
            loop {
                self.find_n_do_rule(&mut s, m, seg, smap, &mut rules);
                if m.status != MStatus::Finished {
                    return false;
                }
                if s != NIL
                    && (s == smap.highwater() || smap.highpassed() || {
                        lc -= 1;
                        lc == 0
                    })
                {
                    if lc == 0 {
                        s = smap.highwater();
                    }
                    lc = i32::from(self.i_max_loop);
                    if s != NIL {
                        let n = seg.s(s).next;
                        smap.set_highwater(n);
                    }
                }
                if s == NIL {
                    break;
                }
            }
        }
        let collisions = self.num_coll_runs != 0 || self.kern_colls != 0;
        if !collisions || !seg.has_collision_info() {
            return true;
        }
        let dir = smap.dir();
        if self.num_coll_runs != 0 {
            if seg.flags & SEG_INITCOLLISIONS == 0 {
                seg.position_slots(None, NIL, NIL, dir != 0, true);
            }
            if !self.collision_shift(seg, i32::from(dir)) {
                return false;
            }
        }
        if self.kern_colls != 0 && !self.collision_kern(seg, i32::from(dir)) {
            return false;
        }
        Self::collision_finish(seg);
        true
    }

    /// `runFSM(fsm, slot)`.
    fn run_fsm(&self, seg: &Segment, smap: &mut SlotMap, rules: &mut Rules, slot: u32) -> bool {
        // fsm.reset(slot, max_pre_ctxt)
        rules.clear();
        let mut slot = slot;
        let mut ctxt: u16 = 0;
        while ctxt != u16::from(self.max_pre_ctxt) && seg.s(slot).prev != NIL {
            ctxt += 1;
            slot = seg.s(slot).prev;
        }
        smap.reset(seg, slot, ctxt);
        if smap.context() < u16::from(self.min_pre_ctxt) {
            return false;
        }
        let mut state =
            self.start_states[usize::from(self.max_pre_ctxt) - usize::from(smap.context())];
        let mut free_slots = MAX_SLOTS as u8;
        loop {
            smap.push_slot(slot);
            let gid = seg.s(slot).glyphid;
            if gid >= self.num_glyphs
                || self.cols[usize::from(gid)] == 0xffff
                || {
                    free_slots -= 1;
                    free_slots == 0
                }
                || state >= self.num_transition
            {
                return free_slots != 0;
            }
            let row = usize::from(state) * usize::from(self.num_columns);
            state = self.transitions[row + usize::from(self.cols[usize::from(gid)])];
            if state >= self.success_start {
                let (b, e) = self.states[usize::from(state)];
                rules.accumulate(&self.rules, &self.rule_map[b as usize..e as usize]);
            }
            slot = seg.s(slot).next;
            if state == 0 || slot == NIL {
                break;
            }
        }
        smap.push_slot(slot);
        true
    }

    /// `findNDoRule(slot, m, fsm)`.
    fn find_n_do_rule(
        &self,
        slot: &mut u32,
        m: &mut Machine,
        seg: &mut Segment,
        smap: &mut SlotMap,
        rules: &mut Rules,
    ) {
        if self.run_fsm(seg, smap, rules, *slot) {
            let list: Vec<u16> = rules.as_slice().to_vec();
            let mut k = 0;
            while k < list.len()
                && !self.test_constraint(&self.rules[usize::from(list[k])], m, seg, smap)
            {
                k += 1;
                if m.status != MStatus::Finished {
                    return;
                }
            }
            if k < list.len() {
                let rule = &self.rules[usize::from(list[k])];
                let adv = Self::do_action(&rule.action, slot, m, seg, smap);
                if m.status != MStatus::Finished {
                    return;
                }
                if rule.action.delete {
                    smap.collect_garbage(seg, slot);
                }
                Self::adjust_slot(adv, slot, seg, smap);
                return;
            }
        }
        *slot = seg.s(*slot).next;
    }

    /// `testPassConstraint(m)`.
    fn test_pass_constraint(&self, m: &mut Machine, seg: &mut Segment, smap: &mut SlotMap) -> bool {
        if !self.pconstraint.is_some() {
            return true;
        }
        let f = seg.first;
        smap.reset(seg, f, 0);
        smap.push_slot(f);
        let mut map = 1usize;
        let ret = m.run_code(&self.pconstraint, seg, smap, &mut map);
        ret != 0 && m.status == MStatus::Finished
    }

    /// `testConstraint(r, m)`.
    fn test_constraint(
        &self,
        r: &Rule,
        m: &mut Machine,
        seg: &mut Segment,
        smap: &mut SlotMap,
    ) -> bool {
        let curr_context = i32::from(smap.context());
        let pre = i32::from(r.pre_context);
        if (i32::from(r.sort) + curr_context - pre) as u32 > smap.size() as u32
            || curr_context - pre < 0
        {
            return false;
        }
        let mut map = (1 + curr_context - pre) as usize;
        if smap.slot_map[map + usize::from(r.sort) - 1] == NIL {
            return false;
        }
        if !r.constraint.is_some() {
            return true;
        }
        let mut n = r.sort;
        while n != 0 {
            if smap.slot_map.get(map).copied().unwrap_or(NIL) != NIL {
                let ret = m.run_code(&r.constraint, seg, smap, &mut map);
                if ret == 0 || m.status != MStatus::Finished {
                    return false;
                }
            }
            n -= 1;
            map += 1;
        }
        true
    }

    /// `doAction(codeptr, slot_out, m)`.
    fn do_action(
        code: &Code,
        slot_out: &mut u32,
        m: &mut Machine,
        seg: &mut Segment,
        smap: &mut SlotMap,
    ) -> i32 {
        if !code.is_some() {
            return 0;
        }
        let mut map = 1 + usize::from(smap.context());
        smap.set_highpassed(false);
        let ret = m.run_code(code, seg, smap, &mut map);
        if m.status != MStatus::Finished {
            *slot_out = NIL;
            smap.set_highwater(NIL);
            return 0;
        }
        *slot_out = smap.slot_map[map];
        ret
    }

    /// `adjustSlot(delta, slot_out, smap)`.
    fn adjust_slot(delta: i32, slot_out: &mut u32, seg: &Segment, smap: &mut SlotMap) {
        let mut delta = delta;
        if *slot_out == NIL {
            if smap.highpassed() || *slot_out == smap.highwater() {
                *slot_out = seg.last;
                delta += 1;
                if smap.highwater() == NIL || smap.highwater() == *slot_out {
                    smap.set_highpassed(false);
                }
            } else {
                *slot_out = seg.first;
                delta -= 1;
            }
        }
        if delta < 0 {
            loop {
                delta += 1;
                if !(delta <= 0 && *slot_out != NIL) {
                    break;
                }
                *slot_out = seg.s(*slot_out).prev;
                if smap.highpassed() && smap.highwater() == *slot_out {
                    smap.set_highpassed(false);
                }
            }
        } else if delta > 0 {
            loop {
                delta -= 1;
                if !(delta >= 0 && *slot_out != NIL) {
                    break;
                }
                if *slot_out == smap.highwater() && *slot_out != NIL {
                    smap.set_highpassed(true);
                }
                *slot_out = seg.s(*slot_out).next;
            }
        }
    }

    fn cflags(seg: &Segment, s: u32) -> u16 {
        seg.collision(s).map_or(0, |c| c.flags)
    }

    /// `collisionShift(seg, dir)`.
    fn collision_shift(&self, seg: &mut Segment, dir: i32) -> bool {
        let mut coll = ShiftCollider::new();
        let mut has_collisions;
        let mut start = seg.first;
        let mut end;
        let mut moved = false;
        while start != NIL {
            has_collisions = false;
            end = NIL;
            // phase 1
            let mut s = start;
            while s != NIL {
                let f = Self::cflags(seg, s);
                if f & (COLL_FIX | COLL_KERN) == COLL_FIX
                    && !self.resolve_collisions(
                        seg,
                        s,
                        start,
                        &mut coll,
                        false,
                        dir,
                        &mut moved,
                        &mut has_collisions,
                    )
                {
                    return false;
                }
                if s != start && Self::cflags(seg, s) & COLL_END != 0 {
                    end = seg.s(s).next;
                    break;
                }
                s = seg.s(s).next;
            }
            // phase 2
            for _ in 0..i32::from(self.num_coll_runs) - 1 {
                if has_collisions || moved {
                    if has_collisions {
                        has_collisions = false;
                        let lend = if end != NIL {
                            seg.s(end).prev
                        } else {
                            seg.last
                        };
                        let lstart = seg.s(start).prev;
                        let mut s = lend;
                        while s != lstart {
                            let f = Self::cflags(seg, s);
                            if f & (COLL_FIX | COLL_KERN | COLL_ISCOL) == (COLL_FIX | COLL_ISCOL) {
                                if !self.resolve_collisions(
                                    seg,
                                    s,
                                    lend,
                                    &mut coll,
                                    true,
                                    dir,
                                    &mut moved,
                                    &mut has_collisions,
                                ) {
                                    return false;
                                }
                                if let Some(c) = seg.collision_mut(s) {
                                    c.flags |= COLL_TEMPLOCK;
                                }
                            }
                            s = seg.s(s).prev;
                        }
                    }
                    if moved {
                        moved = false;
                        let mut s = start;
                        while s != end {
                            let f = Self::cflags(seg, s);
                            if f & (COLL_FIX | COLL_TEMPLOCK | COLL_KERN) == COLL_FIX
                                && !self.resolve_collisions(
                                    seg,
                                    s,
                                    start,
                                    &mut coll,
                                    false,
                                    dir,
                                    &mut moved,
                                    &mut has_collisions,
                                )
                            {
                                return false;
                            } else if Self::cflags(seg, s) & COLL_TEMPLOCK != 0
                                && let Some(c) = seg.collision_mut(s)
                            {
                                c.flags &= !COLL_TEMPLOCK;
                            }
                            s = seg.s(s).next;
                        }
                    }
                }
            }
            if end == NIL {
                break;
            }
            start = NIL;
            let mut s = seg.s(end).prev;
            while s != NIL {
                if Self::cflags(seg, s) & COLL_START != 0 {
                    start = s;
                    break;
                }
                s = seg.s(s).next;
            }
        }
        true
    }

    /// `collisionKern(seg, dir)`.
    fn collision_kern(&self, seg: &mut Segment, dir: i32) -> bool {
        let mut start = seg.first;
        let mut ymin = 1e38f32;
        let mut ymax = -1e38f32;
        let mut s = seg.first;
        while s != NIL {
            let gid = seg.s(s).glyphid;
            if !seg.face.glyphs.check(gid) {
                return false;
            }
            let c = seg.collision(s).copied().unwrap_or_default();
            let bbox = seg.face.glyphs.bbox(gid);
            let y = seg.s(s).position.y + c.shift.y;
            if c.flags & COLL_ISSPACE == 0 {
                ymax = gmax(y + bbox.tr.y, ymax);
                ymin = gmin(y + bbox.bl.y, ymin);
            }
            if start != NIL && c.flags & (COLL_KERN | COLL_FIX) == (COLL_KERN | COLL_FIX) {
                self.resolve_kern(seg, s, dir, &mut ymin, &mut ymax);
            }
            let f = Self::cflags(seg, s);
            if f & COLL_END != 0 {
                start = NIL;
            }
            if f & COLL_START != 0 {
                start = s;
            }
            s = seg.s(s).next;
        }
        true
    }

    /// `collisionFinish(seg)`.
    fn collision_finish(seg: &mut Segment) {
        let mut s = seg.first;
        while s != NIL {
            if let Some(c) = seg.collision_mut(s)
                && (c.shift.x != 0.0 || c.shift.y != 0.0)
            {
                let new_offset = c.shift;
                c.offset = new_offset + c.offset;
                c.shift = Position::default();
            }
            s = seg.s(s).next;
        }
    }

    /// `inKernCluster(seg, s)`.
    fn in_kern_cluster(seg: &Segment, s: u32) -> bool {
        if Self::cflags(seg, s) & COLL_KERN != 0 {
            return true;
        }
        let mut s = s;
        while seg.s(s).parent != NIL {
            s = seg.s(s).parent;
            if Self::cflags(seg, s) & COLL_KERN != 0 {
                return true;
            }
        }
        false
    }

    /// `resolveCollisions(seg, slotFix, start, coll, isRev, dir, moved,
    /// hasCol)`.
    #[allow(clippy::too_many_arguments)]
    fn resolve_collisions(
        &self,
        seg: &mut Segment,
        slot_fix: u32,
        start: u32,
        coll: &mut ShiftCollider,
        is_rev: bool,
        dir: i32,
        moved: &mut bool,
        has_col: &mut bool,
    ) -> bool {
        let c_fix = seg.collision(slot_fix).copied().unwrap_or_default();
        if !coll.init_slot(
            seg,
            slot_fix,
            &c_fix.limit,
            f32::from(c_fix.margin),
            f32::from(c_fix.margin_wt),
            c_fix.shift,
            c_fix.offset,
            dir,
        ) {
            return false;
        }
        let mut collides = false;
        let mut ignore_for_kern = !is_rev;
        let rtl = dir & 1 != 0;
        let base = seg.find_root(slot_fix);
        let mut nbor = start;
        while nbor != NIL {
            let c_nbor = seg.collision(nbor).copied().unwrap_or_default();
            let same_cluster = seg.is_child_of(nbor, base);
            if nbor != slot_fix
                && !c_nbor.ignore()
                && (nbor == base || same_cluster || !Self::in_kern_cluster(seg, nbor))
                && (!is_rev
                    || c_nbor.flags & COLL_FIX == 0
                    || (c_nbor.flags & COLL_KERN != 0 && !same_cluster)
                    || c_nbor.flags & COLL_ISCOL != 0)
                && !coll.merge_slot(
                    seg,
                    nbor,
                    &c_nbor,
                    c_nbor.shift,
                    !ignore_for_kern,
                    same_cluster,
                    &mut collides,
                    false,
                )
            {
                return false;
            } else if nbor == slot_fix {
                ignore_for_kern = !ignore_for_kern;
            }
            if nbor != start && c_nbor.flags & (if is_rev { COLL_START } else { COLL_END }) != 0 {
                break;
            }
            nbor = if is_rev {
                seg.s(nbor).prev
            } else {
                seg.s(nbor).next
            };
        }
        let mut is_col = false;
        let cur_shift = seg
            .collision(slot_fix)
            .map_or(Position::default(), |c| c.shift);
        if collides || cur_shift.x != 0.0 || cur_shift.y != 0.0 {
            let shift = coll.resolve(&mut is_col);
            if shift.x.abs() < 1e38 && shift.y.abs() < 1e38 {
                let ct = f32::from(self.col_threshold);
                if sqr(shift.x - cur_shift.x) + sqr(shift.y - cur_shift.y) >= ct * ct {
                    *moved = true;
                }
                if let Some(c) = seg.collision_mut(slot_fix) {
                    c.shift = shift;
                }
                let child = seg.s(slot_fix).child;
                if child != NIL {
                    let mut bbox = Rect::default();
                    let here = seg.s(slot_fix).position + shift;
                    let mut cluster_min = here.x;
                    seg.finalise_slot(
                        child,
                        None,
                        here,
                        &mut bbox,
                        0,
                        &mut cluster_min,
                        rtl,
                        false,
                        0,
                    );
                }
            }
        }
        if let Some(c) = seg.collision_mut(slot_fix) {
            if is_col {
                c.flags |= COLL_ISCOL | COLL_KNOWN;
            } else {
                c.flags = (c.flags & !COLL_ISCOL) | COLL_KNOWN;
            }
        }
        *has_col |= is_col;
        true
    }

    /// `resolveKern(seg, slotFix, start, dir, ymin, ymax)`.
    fn resolve_kern(
        &self,
        seg: &mut Segment,
        slot_fix: u32,
        dir: i32,
        ymin: &mut f32,
        ymax: &mut f32,
    ) -> f32 {
        let mut curr_space = 0.0f32;
        let mut collides = false;
        let mut space_count: u32 = 0;
        let base = seg.find_root(slot_fix);
        let c_fix = seg.collision(base).copied().unwrap_or_default();
        let gc = &seg.face.glyphs;
        let bbb = gc.bbox(seg.s(slot_fix).glyphid);
        let by = seg.s(slot_fix).position.y + c_fix.shift.y;
        if base != slot_fix {
            if let Some(c) = seg.collision_mut(base) {
                c.flags |= COLL_KERN | COLL_FIX;
            }
            return 0.0;
        }
        let mut seen_end = c_fix.flags & COLL_END != 0;
        let mut is_init = false;
        let mut coll = KernCollider::new();
        *ymax = gmax(by + bbb.tr.y, *ymax);
        *ymin = gmin(by + bbb.bl.y, *ymin);
        let mut nbor = seg.s(slot_fix).next;
        while nbor != NIL {
            let gid = seg.s(nbor).glyphid;
            if !seg.face.glyphs.check(gid) {
                return 0.0;
            }
            let bb = seg.face.glyphs.bbox(gid);
            let c_nbor = seg.collision(nbor).copied().unwrap_or_default();
            let nby = seg.s(nbor).position.y + c_nbor.shift.y;
            if seg.is_child_of(nbor, base) {
                *ymax = gmax(nby + bb.tr.y, *ymax);
                *ymin = gmin(nby + bb.bl.y, *ymin);
                nbor = seg.s(nbor).next;
                continue;
            }
            if (bb.bl.y == 0.0 && bb.tr.y == 0.0) || c_nbor.flags & COLL_ISSPACE != 0 {
                if self.kern_colls == IN_WORD {
                    break;
                }
                curr_space += seg.s(nbor).advance.x;
                space_count += 1;
            } else {
                space_count = 0;
                if nbor != slot_fix && !c_nbor.ignore() {
                    seen_end = true;
                    if !is_init {
                        if !coll.init_slot(
                            seg,
                            slot_fix,
                            &c_fix.limit,
                            f32::from(c_fix.margin),
                            c_fix.shift,
                            c_fix.offset,
                            dir,
                            *ymin,
                            *ymax,
                        ) {
                            return 0.0;
                        }
                        is_init = true;
                    }
                    collides |= coll.merge_slot(seg, nbor, c_nbor.shift, curr_space, dir);
                }
            }
            if c_nbor.flags & COLL_END != 0 {
                if seen_end && space_count < 2 {
                    break;
                }
                seen_end = true;
            }
            nbor = seg.s(nbor).next;
        }
        if collides {
            let mv = coll.resolve(dir);
            coll.shift(mv, dir);
            let cur_shift = seg
                .collision(slot_fix)
                .map_or(Position::default(), |c| c.shift);
            let delta = seg.s(slot_fix).advance + mv - cur_shift;
            seg.sm(slot_fix).advance = delta;
            if let Some(c) = seg.collision_mut(slot_fix) {
                c.shift = mv;
            }
            return mv.x;
        }
        0.0
    }
}
