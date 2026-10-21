// Copyright 2012, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Justifier.cpp), LGPL-2.1-or-later.

//! Justification (`gr_seg_justify`): stretching or shrinking a run of
//! slots to a width by the font's justification attributes, then the
//! justification passes.

use alloc::vec::Vec;

use crate::font::Font;
use crate::position::{Position, Rect};
use crate::segment::{NIL, Segment};

/// `gr_justStartInline`, `gr_justEndInline`.
pub const JUST_START_INLINE: u32 = 1;
pub const JUST_END_INLINE: u32 = 2;

#[derive(Clone, Copy, Default)]
struct JustifyTotal {
    num_glyphs: i32,
    stretch: i32,
    shrink: i32,
    step: i32,
    weight: i32,
}

impl Segment<'_> {
    /// `Segment::justify(pSlot, font, width, flags, pFirst, pLast)`.
    #[allow(clippy::too_many_lines)]
    pub fn justify(&mut self, p_slot: u32, font: Option<&Font>, width: f32, jflags: u32, p_first: u32, p_last: u32) -> f32 {
        let mut end = self.last;
        let mut curr_width = 0.0f32;
        let scale = font.map_or(1.0, |f| f.scale);
        let mut width = width;
        let (mut p_slot, mut p_first, mut p_last) = (p_slot, p_first, p_last);
        if width < 0.0 && self.silf.flags == 0 {
            return width;
        }
        let silf_dir = i32::from(self.silf.dir);
        let flip = i32::from(self.dir & 1) != silf_dir && self.silf.b_pass != self.silf.num_passes;
        if flip {
            self.reverse_slots();
            core::mem::swap(&mut p_first, &mut p_last);
        }
        if p_first == NIL {
            p_first = p_slot;
        }
        while !self.s(p_first).is_base() {
            p_first = self.s(p_first).parent;
        }
        if p_last == NIL {
            p_last = self.last;
        }
        while !self.s(p_last).is_base() {
            p_last = self.s(p_last).parent;
        }
        let base = self.s(p_first).position.x / scale;
        width /= scale;
        if jflags & JUST_END_INLINE == 0 {
            while p_last != p_first && p_last != NIL {
                let b: Rect = self.face.glyphs.glyph(self.s(p_last).glyph()).bbox;
                if b.bl.x != 0.0 || b.bl.y != 0.0 || b.tr.x != 0.0 || b.tr.y == 0.0 {
                    break;
                }
                p_last = self.s(p_last).prev;
            }
        }
        if p_last != NIL {
            end = self.s(p_last).sibling;
        }
        if p_first != NIL {
            p_first = self.s(p_first).sibling;
        }
        let mut icount = 0;
        let mut num_levels = i32::from(self.silf.num_justs);
        if num_levels == 0 {
            let mut s = p_slot;
            while s != NIL && s != end {
                let c = self.charinfo[self.s(s).before as usize].char;
                if Segment::is_whitespace(c) {
                    self.set_justify(s, 0, 3, 1);
                    self.set_justify(s, 0, 2, 1);
                    self.set_justify(s, 0, 0, -1);
                    icount += 1;
                }
                s = self.s(s).sibling;
            }
            if icount == 0 {
                let mut s = p_slot;
                while s != NIL && s != end {
                    self.set_justify(s, 0, 3, 1);
                    self.set_justify(s, 0, 2, 1);
                    self.set_justify(s, 0, 0, -1);
                    s = self.s(s).sibling;
                }
            }
            num_levels += 1;
        }
        let mut stats: Vec<JustifyTotal> = alloc::vec![JustifyTotal::default(); num_levels as usize];
        let mut s = p_first;
        while s != NIL && s != end {
            let w = self.s(s).position.x / scale + self.s(s).advance.x - base;
            if w > curr_width {
                curr_width = w;
            }
            for (j, st) in stats.iter_mut().enumerate() {
                st.num_glyphs += 1;
                st.stretch += self.get_justify(s, j as u8, 0);
                st.shrink += self.get_justify(s, j as u8, 1);
                st.step += self.get_justify(s, j as u8, 2);
                st.weight += self.get_justify(s, j as u8, 3);
            }
            self.sm(s).just = 0.0;
            s = self.s(s).sibling;
        }
        let mut i = if width < 0.0 { -1 } else { num_levels - 1 };
        while i >= 0 {
            let mut t_weight = stats[i as usize].weight;
            if t_weight == 0 {
                i -= 1;
                continue;
            }
            let lv = i as u8;
            loop {
                let mut error = 0.0f32;
                let diff = width - curr_width;
                let diffpw = diff / t_weight as f32;
                t_weight = 0;
                let mut s = p_first;
                while s != NIL && s != end {
                    let w = self.get_justify(s, lv, 3);
                    let mut pref = diffpw * w as f32 + error;
                    let mut step = self.get_justify(s, lv, 2);
                    if step == 0 {
                        step = 1;
                    }
                    if pref > 0.0 {
                        let mut max = f32::from(self.get_justify(s, lv, 0) as u16);
                        if i == 0 {
                            max -= self.s(s).just;
                        }
                        if pref > max {
                            pref = max;
                        } else {
                            t_weight += w;
                        }
                    } else {
                        let mut max = f32::from(self.get_justify(s, lv, 1) as u16);
                        if i == 0 {
                            max += self.s(s).just;
                        }
                        if -pref > max {
                            pref = -max;
                        } else {
                            t_weight += w;
                        }
                    }
                    let actual = (pref / step as f32) as i32 * step;
                    if actual != 0 {
                        error += diffpw * w as f32 - actual as f32;
                        if i == 0 {
                            let j = self.s(s).just;
                            self.sm(s).just = j + actual as f32;
                        } else {
                            self.set_justify(s, lv, 4, actual as i16);
                        }
                    }
                    s = self.s(s).sibling;
                }
                curr_width += diff - error;
                if !(i == 0 && error.abs() as i32 > 0 && t_weight != 0) {
                    break;
                }
            }
            i -= 1;
        }
        let old_first = self.first;
        let old_last = self.last;
        if self.silf.flags & 1 != 0 {
            p_slot = self.add_line_end(p_slot);
            self.first = p_slot;
            p_last = self.add_line_end(end);
            self.last = p_last;
            if self.first == NIL || self.last == NIL {
                return -1.0;
            }
        } else {
            self.first = p_slot;
            self.last = p_last;
        }
        if self.silf.j_pass != self.silf.p_pass && (width >= 0.0 || self.silf.flags & 1 != 0) {
            let silf = self.silf;
            silf.run_graphite(self, silf.j_pass, silf.p_pass, false);
        }
        let dir = self.dir;
        let res: Position = self.position_slots(font, p_slot, p_last, dir != 0, true);
        if self.silf.flags & 1 != 0 {
            if self.first != NIL {
                let f = self.first;
                self.del_line_end(f);
            }
            if self.last != NIL {
                let l = self.last;
                self.del_line_end(l);
            }
        }
        self.first = old_first;
        self.last = old_last;
        if flip {
            self.reverse_slots();
        }
        res.x
    }
}
