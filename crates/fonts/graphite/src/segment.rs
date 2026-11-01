// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Segment.cpp, src/Slot.cpp,
// src/inc/Segment.h, src/inc/Slot.h, src/inc/CharInfo.h),
// LGPL-2.1-or-later.

//! The segment: its characters, and its slots as a doubly linked list in
//! an arena (a slot is its index, [`NIL`] for none). Slots are allocated
//! and freed as graphite2 allocates them (blocks, a free list), and a
//! slot copy copies every field, links included, as `memcpy` does.

use alloc::vec::Vec;

use crate::collider::SlotCollision;
use crate::face::Face;
use crate::feature_map::Features;
use crate::font::Font;
use crate::position::{Position, Rect};
use crate::rule::SlotMap;
use crate::silf::Silf;

/// No slot (a null `Slot *`).
pub const NIL: u32 = u32::MAX;

/// `MAX_SEG_GROWTH_FACTOR`.
pub(crate) const MAX_SEG_GROWTH_FACTOR: usize = 64;

/// Slot flags.
pub(crate) const DELETED: u8 = 1;
pub(crate) const INSERTED: u8 = 2;
pub(crate) const COPIED: u8 = 4;

/// `SlotJustify::NUMJUSTPARAMS`.
pub(crate) const NUMJUSTPARAMS: usize = 5;

/// `Segment::SEG_INITCOLLISIONS`, `SEG_HASCOLLISIONS`.
pub(crate) const SEG_INITCOLLISIONS: u8 = 1;
pub(crate) const SEG_HASCOLLISIONS: u8 = 2;

/// `gr_attrCode`.
#[allow(dead_code)]
pub(crate) mod attr {
    pub const ADV_X: u8 = 0;
    pub const ADV_Y: u8 = 1;
    pub const ATT_TO: u8 = 2;
    pub const ATT_X: u8 = 3;
    pub const ATT_Y: u8 = 4;
    pub const ATT_GPT: u8 = 5;
    pub const ATT_X_OFF: u8 = 6;
    pub const ATT_Y_OFF: u8 = 7;
    pub const ATT_WITH_X: u8 = 8;
    pub const ATT_WITH_Y: u8 = 9;
    pub const WITH_GPT: u8 = 10;
    pub const ATT_WITH_X_OFF: u8 = 11;
    pub const ATT_WITH_Y_OFF: u8 = 12;
    pub const ATT_LEVEL: u8 = 13;
    pub const BREAK: u8 = 14;
    pub const COMP_REF: u8 = 15;
    pub const DIR: u8 = 16;
    pub const INSERT: u8 = 17;
    pub const POS_X: u8 = 18;
    pub const POS_Y: u8 = 19;
    pub const SHIFT_X: u8 = 20;
    pub const SHIFT_Y: u8 = 21;
    pub const USER_DEFN_V1: u8 = 22;
    pub const MEASURE_SOL: u8 = 23;
    pub const MEASURE_EOL: u8 = 24;
    pub const J_STRETCH: u8 = 25;
    pub const J_WIDTH: u8 = 29;
    pub const SEG_SPLIT: u8 = 54;
    pub const USER_DEFN: u8 = 55;
    pub const BIDI_LEVEL: u8 = 56;
    pub const COL_FLAGS: u8 = 57;
    pub const COL_LIMIT_BLX: u8 = 58;
    pub const COL_LIMIT_BLY: u8 = 59;
    pub const COL_LIMIT_TRX: u8 = 60;
    pub const COL_LIMIT_TRY: u8 = 61;
    pub const COL_SHIFT_X: u8 = 62;
    pub const COL_SHIFT_Y: u8 = 63;
    pub const COL_MARGIN: u8 = 64;
    pub const COL_MARGIN_WT: u8 = 65;
    pub const COL_EXCL_GLYPH: u8 = 66;
    pub const COL_EXCL_OFF_X: u8 = 67;
    pub const COL_EXCL_OFF_Y: u8 = 68;
    pub const SEQ_CLASS: u8 = 69;
    pub const SEQ_PROX_CLASS: u8 = 70;
    pub const SEQ_ORDER: u8 = 71;
    pub const SEQ_ABOVE_XOFF: u8 = 72;
    pub const SEQ_ABOVE_WT: u8 = 73;
    pub const SEQ_BELOW_XLIM: u8 = 74;
    pub const SEQ_BELOW_WT: u8 = 75;
    pub const SEQ_VALIGN_HT: u8 = 76;
    pub const SEQ_VALIGN_WT: u8 = 77;
    pub const MAX: u8 = 78;
}

/// A slot (`Slot`): a glyph in the segment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slot {
    pub next: u32,
    pub prev: u32,
    pub glyphid: u16,
    pub realglyphid: u16,
    pub original: u32,
    /// `m_before`, `m_after`: `uint32` read as `int`.
    pub before: i32,
    pub after: i32,
    pub index: u32,
    pub parent: u32,
    pub child: u32,
    pub sibling: u32,
    pub position: Position,
    pub shift: Position,
    pub advance: Position,
    pub attach: Position,
    pub with: Position,
    pub just: f32,
    pub flags: u8,
    pub att_level: u8,
    pub bidi_cls: i8,
    pub bidi_level: u8,
    /// `m_userAttr`: offset of the slot's user attributes in the
    /// segment's store.
    pub user: u32,
    /// `m_justs`: a justification block, or [`NIL`].
    pub justs: u32,
}

impl Slot {
    /// `Slot(user_attrs)`.
    fn new(user: u32) -> Slot {
        Slot {
            next: NIL,
            prev: NIL,
            glyphid: 0,
            realglyphid: 0,
            original: 0,
            before: 0,
            after: 0,
            index: 0,
            parent: NIL,
            child: NIL,
            sibling: NIL,
            position: Position::default(),
            shift: Position::default(),
            advance: Position::default(),
            attach: Position::default(),
            with: Position::default(),
            just: 0.0,
            flags: 0,
            att_level: 0,
            bidi_cls: -1,
            bidi_level: 0,
            user,
            justs: NIL,
        }
    }

    /// `glyph()`: the real glyph of a pseudo glyph.
    pub(crate) fn glyph(&self) -> u16 {
        if self.realglyphid != 0 {
            self.realglyphid
        } else {
            self.glyphid
        }
    }

    pub(crate) fn is_base(&self) -> bool {
        self.parent == NIL
    }

    pub(crate) fn is_deleted(&self) -> bool {
        self.flags & DELETED != 0
    }

    pub(crate) fn is_copied(&self) -> bool {
        self.flags & COPIED != 0
    }

    /// `isInsertBefore`.
    pub(crate) fn is_insert_before(&self) -> bool {
        self.flags & INSERTED == 0
    }

    pub(crate) fn mark_deleted(&mut self, v: bool) {
        if v {
            self.flags |= DELETED;
        } else {
            self.flags &= !DELETED;
        }
    }

    pub(crate) fn mark_copied(&mut self, v: bool) {
        if v {
            self.flags |= COPIED;
        } else {
            self.flags &= !COPIED;
        }
    }

    /// `markInsertBefore`.
    pub(crate) fn mark_insert_before(&mut self, v: bool) {
        if v {
            self.flags &= !INSERTED;
        } else {
            self.flags |= INSERTED;
        }
    }

    /// `origin(pos)`.
    pub(crate) fn set_origin(&mut self, p: Position) {
        self.position = p + self.shift;
    }

    /// `attachOffset`.
    #[allow(dead_code)]
    pub(crate) fn attach_offset(&self) -> Position {
        self.attach - self.with
    }
}

/// `CharInfo`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CharInfo {
    pub char: i32,
    pub before: i32,
    pub after: i32,
    pub base: usize,
    pub feature_id: u8,
    /// `m_break`, an `int8`.
    pub break_weight: i8,
    pub flags: u8,
}

impl Default for CharInfo {
    fn default() -> CharInfo {
        CharInfo {
            char: 0,
            before: -1,
            after: -1,
            base: 0,
            feature_id: 0,
            break_weight: 0,
            flags: 0,
        }
    }
}

/// A segment (`Segment`).
pub struct Segment<'a> {
    pub(crate) advance: Position,
    pub(crate) slots: Vec<Slot>,
    /// The user attributes of every slot, `num_user` (plus none: no
    /// logger) each.
    pub(crate) user_attrs: Vec<i16>,
    /// The justification blocks, `just_size` values each, and their free
    /// list's head.
    pub(crate) justs: Vec<i16>,
    just_next: Vec<u32>,
    free_justs: u32,
    pub(crate) feats: Vec<Features>,
    free_slots: u32,
    pub(crate) charinfo: Vec<CharInfo>,
    pub(crate) collisions: Option<Vec<SlotCollision>>,
    pub(crate) face: &'a Face,
    pub(crate) silf: &'a Silf,
    pub(crate) first: u32,
    pub(crate) last: u32,
    buf_size: usize,
    pub(crate) num_glyphs: usize,
    num_charinfo: usize,
    default_original: i32,
    pub(crate) dir: i8,
    pub(crate) flags: u8,
    pub(crate) pass_bits: u8,
}

impl<'a> Segment<'a> {
    /// `Segment(numchars, face, script, dir)`.
    pub(crate) fn new(numchars: usize, face: &'a Face, silf: &'a Silf, dir: i32) -> Segment<'a> {
        let mut s = Segment {
            advance: Position::default(),
            slots: Vec::new(),
            user_attrs: Vec::new(),
            justs: Vec::new(),
            just_next: Vec::new(),
            free_justs: NIL,
            feats: Vec::new(),
            free_slots: NIL,
            charinfo: alloc::vec![CharInfo::default(); numchars],
            collisions: None,
            face,
            silf,
            first: NIL,
            last: NIL,
            buf_size: numchars + 10,
            num_glyphs: numchars,
            num_charinfo: numchars,
            default_original: 0,
            dir: dir as i8,
            flags: u8::from(silf.flags & 0x20 != 0) << 1,
            pass_bits: if silf.a_pass_bits != 0 { 0xFF } else { 0 },
        };
        let first = s.new_slot();
        s.free_slot(first);
        s.buf_size = log_binary(numchars).wrapping_add(1) as usize;
        s
    }

    pub(crate) fn num_user(&self) -> usize {
        usize::from(self.silf.a_user)
    }

    /// `slotCount()`.
    pub(crate) fn slot_count(&self) -> usize {
        self.num_glyphs
    }

    /// `currdir()`.
    pub(crate) fn currdir(&self) -> bool {
        ((self.dir >> 6) ^ self.dir) & 1 != 0
    }

    /// `mergePassBits(uint8)`.
    pub(crate) fn merge_pass_bits(&mut self, v: u8) {
        self.pass_bits &= v;
    }

    #[inline]
    pub(crate) fn s(&self, i: u32) -> &Slot {
        &self.slots[i as usize]
    }

    #[inline]
    pub(crate) fn sm(&mut self, i: u32) -> &mut Slot {
        &mut self.slots[i as usize]
    }

    /// `glyphAttr(gid, gattr)`.
    pub(crate) fn glyph_attr(&self, gid: u16, gattr: u16) -> i16 {
        match self.face.glyphs.glyph_safe(gid) {
            Some(g) => g.attrs.get(gattr) as i16,
            None => 0,
        }
    }

    /// The user attribute `i` of slot `s`.
    pub(crate) fn user(&self, s: u32, i: usize) -> i16 {
        self.user_attrs[self.s(s).user as usize + i]
    }

    pub(crate) fn set_user(&mut self, s: u32, i: usize, v: i16) {
        let o = self.s(s).user as usize + i;
        self.user_attrs[o] = v;
    }

    /// Copies `from`'s user attributes over `to`'s.
    pub(crate) fn copy_user(&mut self, to: u32, from: u32) {
        let n = self.num_user();
        let (t, f) = (self.s(to).user as usize, self.s(from).user as usize);
        self.user_attrs.copy_within(f..f + n, t);
    }

    /// `newSlot()`.
    pub(crate) fn new_slot(&mut self) -> u32 {
        if self.free_slots == NIL {
            if self.num_glyphs > self.num_charinfo * MAX_SEG_GROWTH_FACTOR {
                return NIL;
            }
            let num_user = self.num_user();
            let base = self.slots.len() as u32;
            for i in 0..self.buf_size as u32 {
                let mut sl = Slot::new(((base + i) as usize * num_user) as u32);
                sl.next = if i + 1 < self.buf_size as u32 {
                    base + i + 1
                } else {
                    NIL
                };
                self.slots.push(sl);
            }
            self.user_attrs.resize(self.slots.len() * num_user, 0);
            self.slots[base as usize].next = NIL;
            self.free_slots = if self.buf_size > 1 { base + 1 } else { NIL };
            return base;
        }
        let res = self.free_slots;
        self.free_slots = self.s(res).next;
        self.sm(res).next = NIL;
        res
    }

    /// `freeSlot(aSlot)`.
    pub(crate) fn free_slot(&mut self, a: u32) {
        if a == NIL {
            return;
        }
        if self.last == a {
            self.last = self.s(a).prev;
        }
        if self.first == a {
            self.first = self.s(a).next;
        }
        let parent = self.s(a).parent;
        if parent != NIL {
            self.remove_child(parent, a);
        }
        loop {
            let c = self.s(a).child;
            if c == NIL {
                break;
            }
            if self.s(c).parent == a {
                self.sm(c).parent = NIL;
                self.remove_child(a, c);
            } else {
                self.sm(a).child = NIL;
            }
        }
        let user = self.s(a).user;
        *self.sm(a) = Slot::new(user);
        let n = self.num_user();
        self.user_attrs[user as usize..user as usize + n].fill(0);
        self.sm(a).next = self.free_slots;
        self.free_slots = a;
    }

    fn just_size(&self) -> usize {
        usize::from(self.silf.num_justs.max(1)) * NUMJUSTPARAMS
    }

    /// `newJustify()`: a block's index.
    pub(crate) fn new_justify(&mut self) -> u32 {
        if self.free_justs == NIL {
            let size = self.just_size();
            let base = self.just_next.len() as u32;
            for i in 0..self.buf_size as u32 {
                self.just_next.push(if i + 1 < self.buf_size as u32 {
                    base + i + 1
                } else {
                    NIL
                });
            }
            self.justs.resize(self.just_next.len() * size, 0);
            self.free_justs = if self.buf_size > 0 { base } else { NIL };
            if self.free_justs == NIL {
                return NIL;
            }
        }
        let res = self.free_justs;
        self.free_justs = self.just_next[res as usize];
        self.just_next[res as usize] = NIL;
        res
    }

    /// `freeJustify`.
    #[allow(dead_code)]
    pub(crate) fn free_justify(&mut self, j: u32) {
        let size = self.just_size();
        self.just_next[j as usize] = self.free_justs;
        self.justs[j as usize * size..(j as usize + 1) * size].fill(0);
        self.free_justs = j;
    }

    /// `appendSlot(id, cid, gid, iFeats, coffset)`.
    pub(crate) fn append_slot(
        &mut self,
        id: usize,
        cid: i32,
        gid: u16,
        ifeats: i32,
        coffset: usize,
    ) {
        let a = self.new_slot();
        if a == NIL {
            return;
        }
        let glyph = self.face.glyphs.glyph_safe(gid);
        let ci = &mut self.charinfo[id];
        ci.char = cid;
        ci.feature_id = ifeats as u8;
        ci.base = coffset;
        ci.break_weight = glyph.map_or(0, |g| g.attrs.get(u16::from(self.silf.a_break))) as i8;
        self.sm(a).child = NIL;
        self.set_glyph(a, gid);
        let s = self.sm(a);
        s.original = id as u32;
        s.before = id as i32;
        s.after = id as i32;
        if self.last != NIL {
            let l = self.last;
            self.sm(l).next = a;
        }
        self.sm(a).prev = self.last;
        self.last = a;
        if self.first == NIL {
            self.first = a;
        }
        if let Some(g) = glyph
            && self.silf.a_pass_bits != 0
        {
            let ab = u16::from(self.silf.a_pass_bits);
            let hi = if self.silf.num_passes > 16 {
                i32::from(g.attrs.get(ab + 1)) << 16
            } else {
                0
            };
            self.pass_bits &= (i32::from(g.attrs.get(ab)) | hi) as u8;
        }
    }

    /// `Slot::setGlyph(seg, glyphid)`.
    pub(crate) fn set_glyph(&mut self, s: u32, gid: u16) {
        let face = self.face;
        let silf = self.silf;
        {
            let sl = self.sm(s);
            sl.glyphid = gid;
            sl.bidi_cls = -1;
        }
        let Some(the) = face.glyphs.glyph_safe(gid) else {
            let sl = self.sm(s);
            sl.realglyphid = 0;
            sl.advance = Position::default();
            return;
        };
        let mut real = the.attrs.get(u16::from(silf.a_pseudo));
        if real > face.glyphs.num_glyphs() {
            real = 0;
        }
        let mut a = the;
        if real != 0 {
            a = face.glyphs.glyph_safe(real).unwrap_or(the);
        }
        let adv = a.advance.x;
        {
            let sl = self.sm(s);
            sl.realglyphid = real;
            sl.advance = Position::new(adv, 0.0);
        }
        if silf.a_pass_bits != 0 {
            let ab = u16::from(silf.a_pass_bits);
            self.merge_pass_bits(the.attrs.get(ab) as u8);
            if silf.num_passes > 16 {
                // (an `int` shifted 16 left, passed as a `uint8`: 0)
                self.merge_pass_bits(((i32::from(the.attrs.get(ab + 1))) << 16) as u8);
            }
        }
    }

    /// `Slot::child(ap)`.
    pub(crate) fn child(&mut self, s: u32, ap: u32) -> bool {
        if s == ap {
            return false;
        }
        let c = self.s(s).child;
        if ap == c {
            return true;
        }
        if c == NIL {
            self.sm(s).child = ap;
            return true;
        }
        self.sibling(c, ap)
    }

    /// `Slot::sibling(ap)`.
    pub(crate) fn sibling(&mut self, s: u32, ap: u32) -> bool {
        let mut s = s;
        loop {
            if s == ap {
                return false;
            }
            let sib = self.s(s).sibling;
            if ap == sib {
                return true;
            }
            if sib == NIL || ap == NIL {
                self.sm(s).sibling = ap;
                return true;
            }
            s = sib;
        }
    }

    /// `Slot::removeChild(ap)`.
    pub(crate) fn remove_child(&mut self, s: u32, ap: u32) -> bool {
        let c = self.s(s).child;
        if s == ap || c == NIL || ap == NIL {
            return false;
        }
        if ap == c {
            let ns = self.s(c).sibling;
            self.sm(c).sibling = NIL;
            self.sm(s).child = ns;
            return true;
        }
        let mut p = c;
        while p != NIL {
            let ps = self.s(p).sibling;
            if ps != NIL && ps == ap {
                let pss = self.s(ps).sibling;
                self.sm(p).sibling = pss;
                self.sm(ap).sibling = NIL;
                return true;
            }
            p = ps;
        }
        false
    }

    /// `Slot::isChildOf(base)`.
    pub(crate) fn is_child_of(&self, s: u32, base: u32) -> bool {
        let mut p = self.s(s).parent;
        while p != NIL {
            if p == base {
                return true;
            }
            p = self.s(p).parent;
        }
        false
    }

    /// `Slot::nextInCluster(s)`.
    pub(crate) fn next_in_cluster(&self, s: u32) -> u32 {
        let mut s = s;
        let sl = self.s(s);
        if sl.child != NIL {
            return sl.child;
        } else if sl.sibling != NIL {
            return sl.sibling;
        }
        loop {
            let base = self.s(s).parent;
            if base == NIL {
                return NIL;
            }
            let bs = self.s(base).sibling;
            if bs != NIL {
                return bs;
            }
            s = base;
        }
    }

    /// `Slot::floodShift(adj, depth)`.
    pub(crate) fn flood_shift(&mut self, s: u32, adj: Position, depth: i32) {
        if depth > 100 {
            return;
        }
        self.sm(s).position += adj;
        let (c, sib) = (self.s(s).child, self.s(s).sibling);
        if c != NIL {
            self.flood_shift(c, adj, depth + 1);
        }
        if sib != NIL {
            self.flood_shift(sib, adj, depth + 1);
        }
    }

    /// `findRoot`.
    pub(crate) fn find_root(&self, s: u32) -> u32 {
        let mut s = s;
        while self.s(s).parent != NIL {
            s = self.s(s).parent;
        }
        s
    }

    /// `collisionInfo(s)`: the slot's collision record (by its index).
    pub(crate) fn collision(&self, s: u32) -> Option<&SlotCollision> {
        self.collisions
            .as_ref()
            .and_then(|c| c.get(self.s(s).index as usize))
    }

    pub(crate) fn collision_mut(&mut self, s: u32) -> Option<&mut SlotCollision> {
        let i = self.s(s).index as usize;
        self.collisions.as_mut().and_then(|c| c.get_mut(i))
    }

    /// `hasCollisionInfo()`.
    pub(crate) fn has_collision_info(&self) -> bool {
        self.flags & SEG_HASCOLLISIONS != 0 && self.collisions.is_some()
    }

    /// `getSlotBidiClass(s)`.
    pub(crate) fn slot_bidi_class(&mut self, s: u32) -> i8 {
        let r = self.s(s).bidi_cls;
        if r != -1 {
            return r;
        }
        let r = self.glyph_attr(self.s(s).glyphid, u16::from(self.silf.a_bidi)) as i8;
        self.sm(s).bidi_cls = r;
        r
    }

    /// `getFeature(index, findex)`.
    pub(crate) fn get_feature(&self, index: usize, findex: u8) -> u32 {
        match self.face.sill.feature(findex) {
            Some(r) => self.feats.get(index).map_or(0, |f| r.value(f)),
            None => 0,
        }
    }

    /// `setFeature(index, findex, val)`.
    pub(crate) fn set_feature(&mut self, index: usize, findex: u8, val: u32) {
        if let Some(r) = self.face.sill.feature(findex) {
            let val = if val > r.max_val() { r.max_val() } else { val };
            if let Some(f) = self.feats.get_mut(index) {
                r.apply(val, f);
            }
        }
    }

    /// `getGlyphMetric(iSlot, metric, attrLevel, rtl)`.
    pub(crate) fn glyph_metric(&mut self, s: u32, metric: u8, attr_level: u8, rtl: bool) -> i32 {
        if attr_level > 0 {
            let r = self.find_root(s);
            self.cluster_metric(r, metric, attr_level, rtl)
        } else {
            self.face.glyph_metric(self.s(s).glyphid, metric)
        }
    }

    /// `Slot::clusterMetric`.
    fn cluster_metric(&mut self, s: u32, metric: u8, attr_level: u8, rtl: bool) -> i32 {
        use crate::glyph_cache::metric as m;
        let g = self.s(s).glyph();
        if g >= self.face.glyphs.num_glyphs() {
            return 0;
        }
        let mut bbox = self.face.glyphs.glyph(g).bbox;
        let mut cluster_min = 0.0f32;
        let res = self.finalise_slot(
            s,
            None,
            Position::default(),
            &mut bbox,
            attr_level,
            &mut cluster_min,
            rtl,
            false,
            0,
        );
        match metric {
            m::LSB | m::BB_LEFT => bbox.bl.x as i32,
            m::RSB => (res.x - bbox.tr.x) as i32,
            m::BB_TOP => bbox.tr.y as i32,
            m::BB_BOTTOM => bbox.bl.y as i32,
            m::BB_RIGHT => bbox.tr.x as i32,
            m::BB_WIDTH => (bbox.tr.x - bbox.bl.x) as i32,
            m::BB_HEIGHT => (bbox.tr.y - bbox.bl.y) as i32,
            m::ADV_WIDTH => res.x as i32,
            m::ADV_HEIGHT => res.y as i32,
            _ => 0,
        }
    }

    /// `Slot::finalise`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finalise_slot(
        &mut self,
        s: u32,
        font: Option<&Font>,
        base: Position,
        bbox: &mut Rect,
        attr_level: u8,
        cluster_min: &mut f32,
        rtl: bool,
        is_final: bool,
        depth: i32,
    ) -> Position {
        let sl = *self.s(s);
        if depth > 100 || (attr_level != 0 && sl.att_level > attr_level) {
            return Position::default();
        }
        let mut scale = font.map_or(1.0, |f| f.scale);
        let mut shift = Position::new(
            sl.shift.x * (i32::from(rtl) * -2 + 1) as f32 + sl.just,
            sl.shift.y,
        );
        let mut t_advance = sl.advance.x + sl.just;
        if is_final && let Some(coll) = self.collision(s) {
            let collshift = coll.offset;
            if coll.flags & crate::collider::COLL_KERN == 0 || rtl {
                shift = shift + collshift;
            }
        }
        let glyph_face = self.face.glyphs.glyph_safe(sl.glyph());
        if let Some(f) = font {
            scale = f.scale;
            shift *= scale;
            if f.is_hinted()
                && let Some(gf) = glyph_face
            {
                t_advance = (sl.advance.x - gf.advance.x + sl.just) * scale + f.advance(sl.glyph());
            } else {
                t_advance *= scale;
            }
        }
        let mut res;
        let mut position = base + shift;
        if sl.parent == NIL {
            res = base + Position::new(t_advance, sl.advance.y * scale);
            *cluster_min = position.x;
        } else {
            position += (sl.attach - sl.with) * scale;
            let t_adv = if sl.advance.x >= 0.5 {
                position.x + t_advance - shift.x
            } else {
                0.0
            };
            res = Position::new(t_adv, 0.0);
            if (sl.advance.x >= 0.5 || position.x < 0.0) && position.x < *cluster_min {
                *cluster_min = position.x;
            }
        }
        self.sm(s).position = position;
        if let Some(gf) = glyph_face {
            let ours = gf.bbox * scale + position;
            *bbox = bbox.widen(&ours);
        }
        let c = sl.child;
        if c != NIL && c != s && self.s(c).parent == s {
            let t = self.finalise_slot(
                c,
                font,
                position,
                bbox,
                attr_level,
                cluster_min,
                rtl,
                is_final,
                depth + 1,
            );
            if (sl.parent == NIL || sl.advance.x >= 0.5) && t.x > res.x {
                res = t;
            }
        }
        let sib = sl.sibling;
        if sl.parent != NIL && sib != NIL && sib != s && self.s(sib).parent == sl.parent {
            let t = self.finalise_slot(
                sib,
                font,
                base,
                bbox,
                attr_level,
                cluster_min,
                rtl,
                is_final,
                depth + 1,
            );
            if t.x > res.x {
                res = t;
            }
        }
        if sl.parent == NIL && *cluster_min < base.x {
            let pos = self.s(s).position;
            let adj = Position::new(pos.x - *cluster_min, 0.0);
            res += adj;
            self.sm(s).position += adj;
            let c = self.s(s).child;
            if c != NIL {
                self.flood_shift(c, adj, 0);
            }
        }
        res
    }

    /// `positionSlots(font, iStart, iEnd, isRtl, isFinal)`.
    pub(crate) fn position_slots(
        &mut self,
        font: Option<&Font>,
        start: u32,
        end: u32,
        is_rtl: bool,
        is_final: bool,
    ) -> Position {
        let mut currpos = Position::default();
        let mut cluster_min;
        let mut bbox = Rect::default();
        let reorder = self.currdir() != is_rtl;
        let (mut start, mut end) = (start, end);
        if reorder {
            self.reverse_slots();
            core::mem::swap(&mut start, &mut end);
        }
        if start == NIL {
            start = self.first;
        }
        if end == NIL {
            end = self.last;
        }
        if start == NIL || end == NIL {
            return currpos;
        }
        if is_rtl {
            let stop = self.s(start).prev;
            let mut s = end;
            while s != NIL && s != stop {
                if self.s(s).is_base() {
                    cluster_min = currpos.x;
                    currpos = self.finalise_slot(
                        s,
                        font,
                        currpos,
                        &mut bbox,
                        0,
                        &mut cluster_min,
                        is_rtl,
                        is_final,
                        0,
                    );
                }
                s = self.s(s).prev;
            }
        } else {
            let stop = self.s(end).next;
            let mut s = start;
            while s != NIL && s != stop {
                if self.s(s).is_base() {
                    cluster_min = currpos.x;
                    currpos = self.finalise_slot(
                        s,
                        font,
                        currpos,
                        &mut bbox,
                        0,
                        &mut cluster_min,
                        is_rtl,
                        is_final,
                        0,
                    );
                }
                s = self.s(s).next;
            }
        }
        if reorder {
            self.reverse_slots();
        }
        currpos
    }

    /// `reverseSlots()`: the slots reversed, keeping diacritics (bidi
    /// class 16) after their bases.
    pub(crate) fn reverse_slots(&mut self) {
        self.dir ^= 64;
        if self.first == self.last {
            return;
        }
        let mut curr = self.first;
        let mut out = NIL;
        while curr != NIL && self.slot_bidi_class(curr) == 16 {
            curr = self.s(curr).next;
        }
        if curr == NIL {
            return;
        }
        let tfirst = self.s(curr).prev;
        let mut tlast = curr;
        while curr != NIL {
            let t;
            if self.slot_bidi_class(curr) == 16 {
                let mut d = self.s(curr).next;
                while d != NIL && self.slot_bidi_class(d) == 16 {
                    d = self.s(d).next;
                }
                d = if d != NIL { self.s(d).prev } else { self.last };
                let p = self.s(out).next;
                if p != NIL {
                    self.sm(p).prev = d;
                } else {
                    tlast = d;
                }
                t = self.s(d).next;
                self.sm(d).next = p;
                self.sm(curr).prev = out;
                self.sm(out).next = curr;
            } else {
                if out != NIL {
                    self.sm(out).prev = curr;
                }
                t = self.s(curr).next;
                self.sm(curr).next = out;
                out = curr;
            }
            curr = t;
        }
        self.sm(out).prev = tfirst;
        if tfirst != NIL {
            self.sm(tfirst).next = out;
        } else {
            self.first = out;
        }
        self.last = tlast;
    }

    /// `linkClusters(s, end)`.
    pub(crate) fn link_clusters(&mut self, s: u32, end: u32) {
        let end = self.s(end).next;
        let mut s = s;
        while s != end && !self.s(s).is_base() {
            s = self.s(s).next;
        }
        let mut ls = s;
        if self.dir & 1 != 0 {
            while s != end {
                if self.s(s).is_base() {
                    self.sibling(s, ls);
                    ls = s;
                }
                s = self.s(s).next;
            }
        } else {
            while s != end {
                if self.s(s).is_base() {
                    self.sibling(ls, s);
                    ls = s;
                }
                s = self.s(s).next;
            }
        }
    }

    /// `associateChars(offset, numChars)`.
    pub(crate) fn associate_chars(&mut self, offset: usize, num: usize) {
        for c in &mut self.charinfo[offset..offset + num] {
            c.before = -1;
            c.after = -1;
        }
        let mut i: i32 = 0;
        let mut s = self.first;
        while s != NIL {
            let mut j = self.s(s).before;
            if j >= 0 {
                let after = self.s(s).after;
                while j <= after {
                    let c = &mut self.charinfo[j as usize];
                    if c.before == -1 || i < c.before {
                        c.before = i;
                    }
                    if c.after < i {
                        c.after = i;
                    }
                    j += 1;
                }
            }
            self.sm(s).index = i as u32;
            i += 1;
            s = self.s(s).next;
        }
        let end = (offset + num) as i32;
        let mut s = self.first;
        while s != NIL {
            let idx = self.s(s).index as i32;
            let mut a = self.s(s).after + 1;
            while a < end && self.charinfo[a as usize].after < 0 {
                self.charinfo[a as usize].after = idx;
                a += 1;
            }
            a -= 1;
            self.sm(s).after = a;
            let mut a = self.s(s).before - 1;
            while a >= offset as i32 && self.charinfo[a as usize].before < 0 {
                self.charinfo[a as usize].before = idx;
                a -= 1;
            }
            a += 1;
            self.sm(s).before = a;
            s = self.s(s).next;
        }
    }

    /// `doMirror(aMirror)`.
    pub(crate) fn do_mirror(&mut self, a_mirror: u16) {
        let mut s = self.first;
        while s != NIL {
            let gid = self.s(s).glyphid;
            let g = self.glyph_attr(gid, a_mirror) as u16;
            if g != 0 && (self.dir & 4 == 0 || self.glyph_attr(gid, a_mirror + 1) == 0) {
                self.set_glyph(s, g);
            }
            s = self.s(s).next;
        }
    }

    /// `initCollisions()`.
    pub(crate) fn init_collisions(&mut self) -> bool {
        let n = self.slot_count();
        self.collisions = Some(alloc::vec![SlotCollision::default(); n]);
        let mut p = self.first;
        while p != NIL {
            let idx = self.s(p).index as usize;
            if idx < n {
                let c = SlotCollision::from_slot(self, p);
                if let Some(c0) = c {
                    self.collisions.as_mut().expect("collisions")[idx] = c0;
                }
            } else {
                return false;
            }
            p = self.s(p).next;
        }
        true
    }

    /// `finalise(font, reverse)`.
    pub(crate) fn finalise(&mut self, font: Option<&Font>, reverse: bool) {
        if self.first == NIL || self.last == NIL {
            return;
        }
        let (f, l) = (self.first, self.last);
        // (the Silf direction byte as a `bool`: any nonzero value)
        self.advance = self.position_slots(font, f, l, self.silf.dir != 0, true);
        if reverse && self.currdir() != (self.dir & 1 != 0) {
            self.reverse_slots();
        }
        let (f, l) = (self.first, self.last);
        self.link_clusters(f, l);
    }

    /// `isWhitespace(cid)`.
    pub(crate) fn is_whitespace(cid: i32) -> bool {
        (0x0009..=0x000D).contains(&cid)
            || matches!(
                cid,
                0x0020
                    | 0x0085
                    | 0x00A0
                    | 0x1680
                    | 0x180E
                    | 0x2028
                    | 0x2029
                    | 0x202F
                    | 0x205F
                    | 0x3000
            )
            || (0x2000..=0x200A).contains(&cid)
    }

    /// `Slot::getJustify(seg, level, subindex)`.
    pub(crate) fn get_justify(&self, s: u32, level: u8, sub: u8) -> i32 {
        let nj = self.silf.num_justs;
        if level != 0 && level >= nj {
            return 0;
        }
        let j = self.s(s).justs;
        if j != NIL {
            let size = self.just_size();
            return i32::from(
                self.justs
                    [j as usize * size + usize::from(level) * NUMJUSTPARAMS + usize::from(sub)],
            );
        }
        if level >= nj {
            return 0;
        }
        let ja = self.silf.justs[usize::from(level)];
        let gid = self.s(s).glyphid;
        match sub {
            0 => i32::from(self.glyph_attr(gid, u16::from(ja.0))),
            1 => i32::from(self.glyph_attr(gid, u16::from(ja.1))),
            2 => i32::from(self.glyph_attr(gid, u16::from(ja.2))),
            3 => i32::from(self.glyph_attr(gid, u16::from(ja.3))),
            _ => 0,
        }
    }

    /// `Slot::setJustify(seg, level, subindex, value)`.
    pub(crate) fn set_justify(&mut self, s: u32, level: u8, sub: u8, value: i16) {
        if level != 0 && level >= self.silf.num_justs {
            return;
        }
        if self.s(s).justs == NIL {
            let j = self.new_justify();
            if j == NIL {
                return;
            }
            // SlotJustify::LoadSlot
            let size = self.just_size();
            let gid = self.s(s).glyphid;
            for i in (0..usize::from(self.silf.num_justs)).rev() {
                let ja = self.silf.justs[i];
                let o = j as usize * size + i * NUMJUSTPARAMS;
                self.justs[o] = self.glyph_attr(gid, u16::from(ja.0));
                self.justs[o + 1] = self.glyph_attr(gid, u16::from(ja.1));
                self.justs[o + 2] = self.glyph_attr(gid, u16::from(ja.2));
                self.justs[o + 3] = self.glyph_attr(gid, u16::from(ja.3));
            }
            self.sm(s).justs = j;
        }
        let size = self.just_size();
        let j = self.s(s).justs as usize;
        self.justs[j * size + usize::from(level) * NUMJUSTPARAMS + usize::from(sub)] = value;
    }

    /// `Slot::getAttr(seg, ind, subindex)`.
    pub(crate) fn get_attr(&self, s: u32, ind: u8, sub: u8) -> i32 {
        use crate::collider as c;
        if (attr::J_STRETCH..attr::J_STRETCH + 20).contains(&ind) && ind != attr::J_WIDTH {
            let indx = ind - attr::J_STRETCH;
            return self.get_justify(s, indx / 5, indx % 5);
        }
        let sl = self.s(s);
        let col = |f: fn(&c::SlotCollision) -> i32| self.collision(s).map_or(0, f);
        match ind {
            attr::ADV_X => sl.advance.x as i32,
            attr::ADV_Y => sl.advance.y as i32,
            attr::ATT_TO => i32::from(sl.parent != NIL),
            attr::ATT_X => sl.attach.x as i32,
            attr::ATT_Y => sl.attach.y as i32,
            attr::ATT_WITH_X => sl.with.x as i32,
            attr::ATT_WITH_Y => sl.with.y as i32,
            attr::ATT_LEVEL => i32::from(sl.att_level),
            attr::BREAK => i32::from(self.charinfo[sl.original as usize].break_weight),
            attr::DIR => i32::from(self.dir & 1),
            attr::INSERT => i32::from(sl.is_insert_before()),
            attr::POS_X => sl.position.x as i32,
            attr::POS_Y => sl.position.y as i32,
            attr::SHIFT_X => sl.shift.x as i32,
            attr::SHIFT_Y => sl.shift.y as i32,
            attr::MEASURE_SOL | attr::MEASURE_EOL => -1,
            attr::J_WIDTH => sl.just as i32,
            attr::USER_DEFN_V1 => {
                if self.num_user() > 0 {
                    i32::from(self.user(s, 0))
                } else {
                    0
                }
            }
            attr::USER_DEFN => {
                if usize::from(sub) < self.num_user() {
                    i32::from(self.user(s, usize::from(sub)))
                } else {
                    0
                }
            }
            attr::SEG_SPLIT => i32::from(self.charinfo[sl.original as usize].flags & 3),
            attr::BIDI_LEVEL => i32::from(sl.bidi_level),
            attr::COL_FLAGS => col(|c| i32::from(c.flags)),
            attr::COL_LIMIT_BLX => col(|c| c.limit.bl.x as i32),
            attr::COL_LIMIT_BLY => col(|c| c.limit.bl.y as i32),
            attr::COL_LIMIT_TRX => col(|c| c.limit.tr.x as i32),
            attr::COL_LIMIT_TRY => col(|c| c.limit.tr.y as i32),
            attr::COL_SHIFT_X => col(|c| c.offset.x as i32),
            attr::COL_SHIFT_Y => col(|c| c.offset.y as i32),
            attr::COL_MARGIN => col(|c| i32::from(c.margin)),
            attr::COL_MARGIN_WT => col(|c| i32::from(c.margin_wt)),
            attr::COL_EXCL_GLYPH => col(|c| i32::from(c.excl_glyph)),
            attr::COL_EXCL_OFF_X => col(|c| c.excl_offset.x as i32),
            attr::COL_EXCL_OFF_Y => col(|c| c.excl_offset.y as i32),
            attr::SEQ_CLASS => col(|c| i32::from(c.seq_class)),
            attr::SEQ_PROX_CLASS => col(|c| i32::from(c.seq_prox_class)),
            attr::SEQ_ORDER => col(|c| i32::from(c.seq_order)),
            attr::SEQ_ABOVE_XOFF => col(|c| i32::from(c.seq_above_xoff)),
            attr::SEQ_ABOVE_WT => col(|c| i32::from(c.seq_above_wt)),
            attr::SEQ_BELOW_XLIM => col(|c| i32::from(c.seq_below_xlim)),
            attr::SEQ_BELOW_WT => col(|c| i32::from(c.seq_below_wt)),
            attr::SEQ_VALIGN_HT => col(|c| i32::from(c.seq_valign_ht)),
            attr::SEQ_VALIGN_WT => col(|c| i32::from(c.seq_valign_wt)),
            _ => 0,
        }
    }

    /// `Slot::setAttr(seg, ind, subindex, value, map)`.
    pub(crate) fn set_attr(&mut self, s: u32, ind: u8, sub: u8, value: i16, map: &SlotMap) {
        use crate::collider::COLL_KNOWN;
        let (mut ind, mut sub) = (ind, sub);
        if ind == attr::USER_DEFN_V1 {
            ind = attr::USER_DEFN;
            sub = 0;
            if self.num_user() == 0 {
                return;
            }
        } else if (attr::J_STRETCH..attr::J_STRETCH + 20).contains(&ind) && ind != attr::J_WIDTH {
            let indx = ind - attr::J_STRETCH;
            self.set_justify(s, indx / 5, indx % 5, value);
            return;
        }
        let v = f32::from(value);
        macro_rules! col {
            ($c:ident => $e:expr) => {{
                if let Some($c) = self.collision_mut(s) {
                    $e;
                    $c.flags &= !COLL_KNOWN;
                }
            }};
        }
        match ind {
            attr::ADV_X => self.sm(s).advance.x = v,
            attr::ADV_Y => self.sm(s).advance.y = v,
            attr::ATT_TO => {
                let idx = value as u16;
                if usize::from(idx) < map.size() && map.get(i32::from(idx)) != NIL {
                    let other = map.get(i32::from(idx));
                    if other == s || other == self.s(s).parent || self.s(other).is_copied() {
                        return;
                    }
                    let parent = self.s(s).parent;
                    if parent != NIL {
                        self.remove_child(parent, s);
                        self.sm(s).parent = NIL;
                    }
                    let mut p = other;
                    let mut count = 0;
                    let mut found = false;
                    while p != NIL {
                        count += 1;
                        if p == s {
                            found = true;
                        }
                        p = self.s(p).parent;
                    }
                    p = self.s(s).child;
                    while p != NIL {
                        count += 1;
                        p = self.s(p).child;
                    }
                    p = self.s(s).sibling;
                    while p != NIL {
                        count += 1;
                        p = self.s(p).sibling;
                    }
                    if count < 100 && !found && self.child(other, s) {
                        self.sm(s).parent = other;
                        if (map.dir() != 0) ^ (idx > u16::from(sub)) {
                            let adv = self.s(s).advance.x;
                            self.sm(s).with = Position::new(adv, 0.0);
                        } else {
                            let adv = self.s(other).advance.x;
                            self.sm(s).attach = Position::new(adv, 0.0);
                        }
                    }
                }
            }
            attr::ATT_X => self.sm(s).attach.x = v,
            attr::ATT_Y => self.sm(s).attach.y = v,
            attr::ATT_WITH_X => self.sm(s).with.x = v,
            attr::ATT_WITH_Y => self.sm(s).with.y = v,
            attr::ATT_LEVEL => self.sm(s).att_level = value as u8,
            attr::BREAK => {
                let o = self.s(s).original as usize;
                self.charinfo[o].break_weight = value as i8;
            }
            attr::INSERT => self.sm(s).mark_insert_before(value != 0),
            attr::SHIFT_X => self.sm(s).shift.x = v,
            attr::SHIFT_Y => self.sm(s).shift.y = v,
            attr::J_WIDTH => self.sm(s).just = v,
            attr::SEG_SPLIT => {
                let o = self.s(s).original as usize;
                self.charinfo[o].flags |= (value & 3) as u8;
            }
            attr::USER_DEFN => self.set_user(s, usize::from(sub), value),
            attr::COL_FLAGS => {
                if let Some(c) = self.collision_mut(s) {
                    c.flags = value as u16;
                }
            }
            attr::COL_LIMIT_BLX => col!(c => c.limit.bl.x = v),
            attr::COL_LIMIT_BLY => col!(c => c.limit.bl.y = v),
            attr::COL_LIMIT_TRX => col!(c => c.limit.tr.x = v),
            attr::COL_LIMIT_TRY => col!(c => c.limit.tr.y = v),
            attr::COL_MARGIN => col!(c => c.margin = value as u16),
            attr::COL_MARGIN_WT => col!(c => c.margin_wt = value as u16),
            attr::COL_EXCL_GLYPH => col!(c => c.excl_glyph = value as u16),
            attr::COL_EXCL_OFF_X => col!(c => c.excl_offset.x = v),
            attr::COL_EXCL_OFF_Y => col!(c => c.excl_offset.y = v),
            attr::SEQ_CLASS => col!(c => c.seq_class = value as u16),
            attr::SEQ_PROX_CLASS => col!(c => c.seq_prox_class = value as u16),
            attr::SEQ_ORDER => col!(c => c.seq_order = value as u16),
            attr::SEQ_ABOVE_XOFF => col!(c => c.seq_above_xoff = value),
            attr::SEQ_ABOVE_WT => col!(c => c.seq_above_wt = value as u16),
            attr::SEQ_BELOW_XLIM => col!(c => c.seq_below_xlim = value),
            attr::SEQ_BELOW_WT => col!(c => c.seq_below_wt = value as u16),
            attr::SEQ_VALIGN_HT => col!(c => c.seq_valign_ht = value as u16),
            attr::SEQ_VALIGN_WT => col!(c => c.seq_valign_wt = value as u16),
            _ => {}
        }
    }

    /// `defaultOriginal()`.
    pub(crate) fn default_original(&self) -> i32 {
        self.default_original
    }

    /// `addLineEnd(nSlot)`.
    pub(crate) fn add_line_end(&mut self, n: u32) -> u32 {
        let e = self.new_slot();
        if e == NIL {
            return NIL;
        }
        let gid = self.silf.g_end_line;
        self.set_glyph(e, gid);
        if n != NIL {
            let np = self.s(n).prev;
            self.sm(e).next = n;
            self.sm(e).prev = np;
            self.sm(n).prev = e;
            self.sm(e).before = self.s(n).before;
            let ep = self.s(e).prev;
            self.sm(e).after = if ep != NIL {
                self.s(ep).after
            } else {
                self.s(n).before
            };
        } else {
            let n = self.last;
            self.sm(e).prev = n;
            self.sm(n).next = e;
            let ep = self.s(e).prev;
            self.sm(e).after = self.s(ep).after;
            self.sm(e).before = self.s(n).after;
        }
        e
    }

    /// `delLineEnd(s)`.
    pub(crate) fn del_line_end(&mut self, s: u32) {
        let n = self.s(s).next;
        let p = self.s(s).prev;
        if n != NIL {
            self.sm(n).prev = p;
            if p != NIL {
                self.sm(p).next = n;
            }
        } else {
            self.sm(p).next = NIL;
        }
        self.free_slot(s);
    }
}

/// `log_binary(v)`: the index of the highest set bit (`mask_over_val`'s
/// population count less one).
fn log_binary(v: usize) -> u32 {
    let mut m = v as u64;
    m |= m >> 1;
    m |= m >> 2;
    m |= m >> 4;
    m |= m >> 8;
    m |= m >> 16;
    m |= m >> 32;
    m.count_ones().wrapping_sub(1)
}
