// Copyright 2011, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/inc/Rule.h), LGPL-2.1-or-later.

//! Rules, the slot map a rule matches over, and the state machine's
//! accumulated rule list.

use crate::code::Code;
use crate::segment::{NIL, Segment};

/// A rule: its constraint and action, its length (`sort`) and how many
/// slots of it precede the current one.
pub(crate) struct Rule {
    pub constraint: Code,
    pub action: Code,
    pub sort: u16,
    pub pre_context: u8,
}

/// `RuleEntry::operator<`: longer rules first, then by index.
#[inline]
pub(crate) fn rule_less(rules: &[Rule], a: u16, b: u16) -> bool {
    let (ls, rs) = (rules[usize::from(a)].sort, rules[usize::from(b)].sort);
    ls > rs || (ls == rs && a < b)
}

/// `SlotMap`: `m_slot_map[0]` is the slot before the matched ones
/// (`smap[-1]`), `smap[n]` is `m_slot_map[n + 1]`.
pub(crate) struct SlotMap {
    pub slot_map: [u32; MAX_SLOTS + 1],
    size: u16,
    precontext: u16,
    highwater: u32,
    max_size: i32,
    dir: u8,
    highpassed: bool,
}

/// `SlotMap::MAX_SLOTS`.
pub(crate) const MAX_SLOTS: usize = 64;
/// `FiniteStateMachine::MAX_RULES`.
pub(crate) const MAX_RULES: usize = 128;

impl SlotMap {
    pub(crate) fn new(dir: u8, max_size: usize) -> SlotMap {
        SlotMap {
            slot_map: [NIL; MAX_SLOTS + 1],
            size: 0,
            precontext: 0,
            highwater: NIL,
            max_size: max_size as i32,
            dir,
            highpassed: false,
        }
    }

    /// `size()`.
    pub(crate) fn size(&self) -> usize {
        usize::from(self.size)
    }

    /// `context()`.
    pub(crate) fn context(&self) -> u16 {
        self.precontext
    }

    /// `operator[](n)`.
    pub(crate) fn get(&self, n: i32) -> u32 {
        self.slot_map.get((n + 1) as usize).copied().unwrap_or(NIL)
    }

    /// `reset(slot, ctxt)`.
    pub(crate) fn reset(&mut self, seg: &Segment, slot: u32, ctxt: u16) {
        self.size = 0;
        self.precontext = ctxt;
        self.slot_map[0] = seg.s(slot).prev;
    }

    /// `pushSlot(slot)`.
    pub(crate) fn push_slot(&mut self, s: u32) {
        self.size += 1;
        self.slot_map[usize::from(self.size)] = s;
    }

    pub(crate) fn highwater(&self) -> u32 {
        self.highwater
    }

    /// `highwater(s)`: also clears `highpassed`.
    pub(crate) fn set_highwater(&mut self, s: u32) {
        self.highwater = s;
        self.highpassed = false;
    }

    pub(crate) fn highpassed(&self) -> bool {
        self.highpassed
    }

    pub(crate) fn set_highpassed(&mut self, v: bool) {
        self.highpassed = v;
    }

    pub(crate) fn dir(&self) -> u8 {
        self.dir
    }

    /// `decMax()`.
    pub(crate) fn dec_max(&mut self) -> i32 {
        self.max_size -= 1;
        self.max_size
    }

    /// `collectGarbage(aSlot)`: frees the deleted and copied slots of the
    /// map (all but its last entry).
    pub(crate) fn collect_garbage(&mut self, seg: &mut Segment, a_slot: &mut u32) {
        // begin() .. end() - 1: m_slot_map[1 ..= size - 1]
        let mut i = 1usize;
        while i < usize::from(self.size) {
            let slot = self.slot_map[i];
            if slot != NIL && (seg.s(slot).is_deleted() || seg.s(slot).is_copied()) {
                if slot == *a_slot {
                    let p = seg.s(slot).prev;
                    *a_slot = if p != NIL { p } else { seg.s(slot).next };
                }
                seg.free_slot(slot);
            }
            i += 1;
        }
    }
}

/// `FiniteStateMachine::Rules`: the rules accumulated, sorted, at most
/// [`MAX_RULES`] (two buffers merged into alternately).
pub(crate) struct Rules {
    buf: [u16; MAX_RULES * 2],
    begin: usize,
    end: usize,
}

impl Rules {
    pub(crate) fn new() -> Rules {
        Rules { buf: [0; MAX_RULES * 2], begin: 0, end: 0 }
    }

    pub(crate) fn clear(&mut self) {
        self.end = self.begin;
    }

    pub(crate) fn as_slice(&self) -> &[u16] {
        &self.buf[self.begin..self.end]
    }

    /// `accumulate_rules(state)`: merges the state's sorted rules in.
    pub(crate) fn accumulate(&mut self, rules: &[Rule], state: &[u16]) {
        if state.is_empty() {
            return;
        }
        let mut lre = self.begin;
        let lend = self.end;
        let mut rre = 0usize;
        let rrend = state.len();
        let mut out = if self.begin == 0 { MAX_RULES } else { 0 };
        let lrend = out + MAX_RULES;
        self.begin = out;
        while lre != lend && out != lrend {
            let (l, r) = (self.buf[lre], state[rre]);
            if rule_less(rules, l, r) {
                self.buf[out] = l;
                out += 1;
                lre += 1;
            } else if rule_less(rules, r, l) {
                self.buf[out] = r;
                out += 1;
                rre += 1;
            } else {
                self.buf[out] = l;
                out += 1;
                lre += 1;
                rre += 1;
            }
            if rre == rrend {
                while lre != lend && out != lrend {
                    self.buf[out] = self.buf[lre];
                    out += 1;
                    lre += 1;
                }
                self.end = out;
                return;
            }
        }
        while rre != rrend && out != lrend {
            self.buf[out] = state[rre];
            out += 1;
            rre += 1;
        }
        self.end = out;
    }
}
