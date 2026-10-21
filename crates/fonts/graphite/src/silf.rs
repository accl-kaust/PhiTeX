// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Silf.cpp, src/inc/Silf.h),
// LGPL-2.1-or-later.

//! A `Silf` subtable: the glyph attribute numbers it uses, its classes,
//! pseudo glyphs and passes; and running the passes over a segment.

use alloc::vec::Vec;

use crate::be::{u16_at, u32_at, u8_at};
use crate::code::PassType;
use crate::face::FaceLoad;
use crate::machine::{MStatus, Machine};
use crate::pass::Pass;
use crate::rule::SlotMap;
use crate::segment::{MAX_SEG_GROWTH_FACTOR, Segment};

const ERROROFFSET: u32 = 0xFFFF_FFFF;

/// `gr_faceinfo`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FaceInfo {
    pub extra_ascent: i16,
    pub extra_descent: i16,
    pub upem: u16,
    pub has_bidi_pass: bool,
    pub justifies: bool,
    pub line_ends: bool,
    pub space_contextuals: u8,
}

pub(crate) struct Silf {
    pub(crate) passes: Vec<Pass>,
    pseudos: Vec<(u32, u16)>,
    class_offsets: Vec<u32>,
    class_data: Vec<u16>,
    /// (stretch, shrink, step, weight) attribute numbers per level.
    pub(crate) justs: Vec<(u8, u8, u8, u8)>,
    pub(crate) num_passes: u8,
    pub(crate) num_justs: u8,
    #[allow(dead_code)]
    pub(crate) s_pass: u8,
    pub(crate) p_pass: u8,
    pub(crate) j_pass: u8,
    pub(crate) b_pass: u8,
    pub(crate) flags: u8,
    pub(crate) dir: u8,
    pub(crate) a_pseudo: u8,
    pub(crate) a_break: u8,
    pub(crate) a_user: u8,
    pub(crate) a_bidi: u8,
    pub(crate) a_mirror: u8,
    pub(crate) a_pass_bits: u8,
    #[allow(dead_code)]
    pub(crate) i_max_comp: u8,
    pub(crate) a_collision: u8,
    #[allow(dead_code)]
    pub(crate) a_lig: u16,
    pub(crate) n_class: u16,
    pub(crate) n_linear: u16,
    pub(crate) g_end_line: u16,
    pub(crate) info: FaceInfo,
}

impl Silf {
    /// `readGraphite(silf, lSilf, face, version)`.
    pub(crate) fn read(d: &[u8], face: &FaceLoad, version: u32) -> Option<Silf> {
        let l = d.len();
        if version >= 0x0006_0000 {
            return None;
        }
        let mut p = 0usize;
        if version >= 0x0003_0000 {
            if l < 28 {
                return None;
            }
            p += 4 + 4;
        } else if l < 20 {
            return None;
        }
        let max_glyph = u16_at(d, p);
        let extra_ascent = u16_at(d, p + 2);
        let extra_descent = u16_at(d, p + 4);
        p += 6;
        let num_passes = u8_at(d, p);
        let s_pass = u8_at(d, p + 1);
        let p_pass = u8_at(d, p + 2);
        let j_pass = u8_at(d, p + 3);
        let b_pass = u8_at(d, p + 4);
        let flags = u8_at(d, p + 5);
        p += 6 + 2;
        let a_pseudo = u8_at(d, p);
        let a_break = u8_at(d, p + 1);
        let a_bidi = u8_at(d, p + 2);
        let a_mirror = u8_at(d, p + 3);
        let a_pass_bits = u8_at(d, p + 4);
        let num_justs = u8_at(d, p + 5);
        p += 6;
        if max_glyph >= face.glyphs.num_glyphs() || p + usize::from(num_justs) * 8 >= l {
            return None;
        }
        let mut justs = Vec::with_capacity(usize::from(num_justs));
        for _ in 0..num_justs {
            justs.push((u8_at(d, p), u8_at(d, p + 1), u8_at(d, p + 2), u8_at(d, p + 3)));
            p += 8;
        }
        if p + 2 + 8 >= l {
            return None;
        }
        let a_lig = u16_at(d, p);
        let a_user = u8_at(d, p + 2);
        let i_max_comp = u8_at(d, p + 3);
        let dir = u8_at(d, p + 4).wrapping_sub(1);
        let a_collision = u8_at(d, p + 5);
        p += 6 + 3;
        let ncrit = usize::from(u8_at(d, p));
        p += 1 + 2 * ncrit;
        p += 1;
        if p >= l {
            return None;
        }
        let nscript = usize::from(u8_at(d, p));
        p += 1 + 4 * nscript;
        if p + 2 + 4 >= l {
            return None;
        }
        let g_end_line = u16_at(d, p);
        p += 2;
        let o_passes = p;
        let passes_start = u32_at(d, p) as usize;
        p += 4;
        let num_attrs = usize::from(face.glyphs.num_attrs());
        if usize::from(a_pseudo) >= num_attrs
            || usize::from(a_break) >= num_attrs
            || usize::from(a_bidi) >= num_attrs
            || usize::from(a_mirror) >= num_attrs
            || (a_collision != 0 && usize::from(a_collision) >= num_attrs.wrapping_sub(5))
            || num_passes > 128
            || passes_start >= l
            || p_pass < s_pass
            || p_pass > num_passes
            || s_pass > num_passes
            || j_pass < p_pass
            || j_pass > num_passes
            || (b_pass != 0xFF && (b_pass < j_pass || b_pass > num_passes))
            || a_lig > 127
        {
            return None;
        }
        p += 4 * usize::from(num_passes);
        if p + 2 >= passes_start {
            return None;
        }
        let num_pseudo = usize::from(u16_at(d, p));
        p += 2 + 6;
        if p + num_pseudo * 6 >= passes_start {
            return None;
        }
        let mut pseudos = Vec::with_capacity(num_pseudo);
        for _ in 0..num_pseudo {
            pseudos.push((u32_at(d, p), u16_at(d, p + 4)));
            p += 6;
        }
        let mut silf = Silf {
            passes: Vec::new(),
            pseudos,
            class_offsets: Vec::new(),
            class_data: Vec::new(),
            justs,
            num_passes,
            num_justs,
            s_pass,
            p_pass,
            j_pass,
            b_pass,
            flags,
            dir,
            a_pseudo,
            a_break,
            a_user,
            a_bidi,
            a_mirror,
            a_pass_bits,
            i_max_comp,
            a_collision,
            a_lig,
            n_class: 0,
            n_linear: 0,
            g_end_line,
            info: FaceInfo::default(),
        };
        let clen = silf.read_class_map(&d[..passes_start], p, version);
        if clen == ERROROFFSET || clen as usize > passes_start - p {
            return None;
        }
        let mut op = o_passes;
        for i in 0..usize::from(num_passes) {
            let pass_start = u32_at(d, op) as usize;
            let pass_end = u32_at(d, op + 4) as usize;
            op += 4;
            if pass_start > pass_end || pass_start < passes_start || pass_end > l {
                return None;
            }
            let pt = if i >= usize::from(j_pass) {
                PassType::Justification
            } else if i >= usize::from(p_pass) {
                PassType::Positioning
            } else if i >= usize::from(s_pass) {
                PassType::Substitute
            } else {
                PassType::LineBreak
            };
            let pass = Pass::read(&d[pass_start..pass_end], pass_start, face, &silf, pt)?;
            silf.passes.push(pass);
        }
        silf.info = FaceInfo {
            extra_ascent: extra_ascent as i16,
            extra_descent: extra_descent as i16,
            upem: face.glyphs.units_per_em(),
            has_bidi_pass: b_pass != 0xFF,
            justifies: num_justs != 0 || j_pass < p_pass,
            line_ends: flags & 1 != 0,
            space_contextuals: (flags >> 2) & 0x7,
        };
        Some(silf)
    }

    /// `readClassMap(p, data_len, version, e)`: the class data's length,
    /// or `ERROROFFSET`. `d` ends where the passes start.
    fn read_class_map(&mut self, d: &[u8], start: usize, version: u32) -> u32 {
        let data_len = d.len() - start;
        if data_len < 4 {
            return ERROROFFSET;
        }
        let mut p = start;
        self.n_class = u16_at(d, p);
        self.n_linear = u16_at(d, p + 2);
        p += 4;
        let nc = usize::from(self.n_class);
        let w = if version >= 0x0004_0000 { 4 } else { 2 };
        if self.n_linear > self.n_class || (nc + 1) * w > data_len - 4 {
            return ERROROFFSET;
        }
        // readClassOffsets<T>: offsets of `T`, `T` arithmetic (a 16-bit
        // difference is an `int`, a 32-bit one wraps)
        let peek = |o: usize| if w == 4 { u32_at(d, o) } else { u32::from(u16_at(d, o)) };
        let cls_off: u32 = if w == 4 { (4 + 4 * (nc + 1)) as u32 } else { u32::from((4 + 2 * (nc + 1)) as u16) };
        let half = |v: u32| -> u32 {
            if w == 4 { v.wrapping_sub(cls_off) / 2 } else { ((v as i32 - cls_off as i32) / 2) as u32 }
        };
        let max_off = half(peek(p + w * nc));
        if peek(p) != cls_off || max_off as usize > data_len.wrapping_sub(cls_off as usize) / 2 {
            return ERROROFFSET;
        }
        self.class_offsets = Vec::with_capacity(nc + 1);
        for _ in 0..=nc {
            let o = half(peek(p));
            p += w;
            self.class_offsets.push(o);
            if o > max_off {
                return ERROROFFSET;
            }
        }
        if (max_off as i32) < i32::from(self.n_linear) + (i32::from(self.n_class) - i32::from(self.n_linear)) * 6 {
            return ERROROFFSET;
        }
        for i in 0..usize::from(self.n_linear) {
            if self.class_offsets[i] > self.class_offsets[i + 1] {
                return ERROROFFSET;
            }
        }
        self.class_data = (0..max_off as usize).map(|k| u16_at(d, p + 2 * k)).collect();
        for i in usize::from(self.n_linear)..nc {
            let o = self.class_offsets[i] as usize;
            if o + 4 > max_off as usize {
                return ERROROFFSET;
            }
            let lk = &self.class_data[o..];
            if lk[0] == 0
                || usize::from(lk[0]) * 2 + o + 4 > max_off as usize
                || u32::from(lk[3]) + u32::from(lk[1]) != u32::from(lk[0])
            {
                return ERROROFFSET;
            }
            if (self.class_offsets[i + 1].wrapping_sub(self.class_offsets[i])) & 1 != 0 {
                return ERROROFFSET;
            }
        }
        max_off
    }

    /// `findPseudo(uid)`.
    pub(crate) fn find_pseudo(&self, uid: u32) -> u16 {
        self.pseudos.iter().find(|p| p.0 == uid).map_or(0, |p| p.1)
    }

    /// `findClassIndex(cid, gid)` (`0xFFFF` for none).
    pub(crate) fn find_class_index(&self, cid: u16, gid: u16) -> u16 {
        if cid > self.n_class {
            return 0xFFFF;
        }
        let Some(&off) = self.class_offsets.get(usize::from(cid)) else {
            return 0xFFFF;
        };
        let off = off as usize;
        let cd = |i: usize| self.class_data.get(i).copied().unwrap_or(0);
        if cid < self.n_linear {
            let n = self.class_offsets[usize::from(cid) + 1] as usize - off;
            for i in 0..n {
                if cd(off + i) == gid {
                    return i as u16;
                }
            }
            0xFFFF
        } else {
            let mut min = off + 4;
            let mut max = min + usize::from(cd(off)) * 2;
            loop {
                let p = min + (((max - min) / 2) & !1);
                if cd(p) > gid {
                    max = p;
                } else {
                    min = p;
                }
                if max - min <= 2 {
                    break;
                }
            }
            if cd(min) == gid { cd(min + 1) } else { 0xFFFF }
        }
    }

    /// `getClassGlyph(cid, index)`.
    pub(crate) fn get_class_glyph(&self, cid: u16, index: u32) -> u16 {
        if cid > self.n_class {
            return 0;
        }
        let Some(&loc) = self.class_offsets.get(usize::from(cid)) else {
            return 0;
        };
        let cd = |i: usize| self.class_data.get(i).copied().unwrap_or(0);
        if cid < self.n_linear {
            if index < self.class_offsets[usize::from(cid) + 1] - loc {
                return cd((index + loc) as usize);
            }
        } else {
            let end = self.class_offsets.get(usize::from(cid) + 1).copied().unwrap_or(0);
            let mut i = loc + 4;
            while i < end {
                if u32::from(cd(i as usize + 1)) == index {
                    return cd(i as usize);
                }
                i += 2;
            }
        }
        0
    }

    /// `runGraphite(seg, firstPass, lastPass, dobidi)`.
    pub(crate) fn run_graphite(&self, seg: &mut Segment, first_pass: u8, last_pass: u8, dobidi: bool) -> bool {
        let max_size = seg.slot_count() * MAX_SEG_GROWTH_FACTOR;
        let mut map = SlotMap::new(self.dir, max_size);
        let mut m = Machine::new();
        let mut lbidi = self.b_pass;
        let mut last_pass = last_pass;
        if last_pass == 0 {
            if first_pass == last_pass && lbidi == 0xFF {
                return true;
            }
            last_pass = self.num_passes;
        }
        if (first_pass < lbidi || (dobidi && first_pass == lbidi))
            && (last_pass >= lbidi || (dobidi && u16::from(last_pass) + 1 == u16::from(lbidi)))
        {
            last_pass = last_pass.wrapping_add(1);
        } else {
            lbidi = 0xFF;
        }
        let mut i = usize::from(first_pass);
        while i < usize::from(last_pass) {
            if i == usize::from(lbidi) {
                if seg.currdir() != (self.dir & 1 != 0) {
                    seg.reverse_slots();
                }
                if self.a_mirror != 0 && seg.dir & 3 == 3 {
                    seg.do_mirror(u16::from(self.a_mirror));
                }
                lbidi = last_pass;
                last_pass = last_pass.wrapping_sub(1);
                continue;
            }
            let pass = &self.passes[i];
            let reverse = lbidi == 0xFF && seg.currdir() != ((self.dir & 1 != 0) ^ pass.is_reverse_dir);
            if (i >= 32 || u32::from(seg.pass_bits) & (1u32 << i) == 0 || pass.num_coll_runs != 0)
                && !pass.run_graphite(&mut m, seg, &mut map, reverse)
            {
                return false;
            }
            if m.status != MStatus::Finished || (seg.slot_count() != 0 && seg.slot_count() > max_size) {
                return false;
            }
            i += 1;
        }
        true
    }
}
