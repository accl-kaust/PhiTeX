// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/FeatureMap.cpp, src/inc/FeatureMap.h,
// src/inc/FeatureVal.h), LGPL-2.1-or-later.

//! The features (`Feat`) and the languages' settings (`Sill`): each
//! feature's bits in a packed vector of `u32`s ([`Features`]).

use alloc::vec::Vec;

use crate::be::{i16_at, u16_at, u32_at};

/// A feature (`FeatureRef`).
#[derive(Clone, Debug)]
pub struct FeatureRef {
    /// (value, name id) per setting.
    pub(crate) settings: Vec<(i16, u16)>,
    mask: u32,
    max: u32,
    id: u32,
    name_id: u16,
    flags: u16,
    bits: u8,
    index: u8,
}

/// `FeatureRef::HIDDEN`.
pub(crate) const HIDDEN: u16 = 0x0800;

/// A feature value vector (`FeatureVal`, `gr_feature_val`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub(crate) vals: Vec<u32>,
}

fn mask_over_val(mut v: u32) -> u32 {
    v |= v >> 1;
    v |= v >> 2;
    v |= v >> 4;
    v |= v >> 8;
    v |= v >> 16;
    v
}

impl FeatureRef {
    fn new(bits_offset: &mut u16, max_val: u32, id: u32, name_id: u16, flags: u16, settings: Vec<(i16, u16)>) -> Self {
        let mut mask = mask_over_val(max_val);
        let need_bits = mask.count_ones() as u16;
        let index = bits_offset.wrapping_add(need_bits) / 32;
        if index > *bits_offset / 32 {
            *bits_offset = index * 32;
        }
        let bits = (*bits_offset % 32) as u8;
        *bits_offset = bits_offset.wrapping_add(need_bits);
        mask <<= bits;
        FeatureRef { settings, mask, max: max_val, id, name_id, flags, bits, index: index as u8 }
    }

    /// `applyValToFeature`.
    pub(crate) fn apply(&self, val: u32, dest: &mut Features) -> bool {
        if val > self.max {
            return false;
        }
        let i = usize::from(self.index);
        if i >= dest.vals.len() {
            dest.vals.resize(i + 1, 0);
        }
        dest.vals[i] &= !self.mask;
        dest.vals[i] |= val << self.bits;
        true
    }

    /// `getFeatureVal`.
    pub(crate) fn value(&self, feats: &Features) -> u32 {
        match feats.vals.get(usize::from(self.index)) {
            Some(&v) => (v & self.mask) >> self.bits,
            None => 0,
        }
    }

    pub(crate) fn id(&self) -> u32 {
        self.id
    }
    pub(crate) fn name_id(&self) -> u16 {
        self.name_id
    }
    pub(crate) fn flags(&self) -> u16 {
        self.flags
    }
    pub(crate) fn max_val(&self) -> u32 {
        self.max
    }
}

/// `SillMap` with its `FeatureMap`.
#[derive(Clone, Debug, Default)]
pub(crate) struct SillMap {
    pub(crate) feats: Vec<FeatureRef>,
    /// Indices into `feats`, ordered by id (`m_pNamedFeats`).
    named: Vec<usize>,
    default: Features,
    langs: Vec<(u32, Features)>,
}

impl SillMap {
    /// `readFace`: `Feat` then `Sill`; `None` where graphite2 fails.
    pub(crate) fn read(feat: Option<&[u8]>, sill: Option<&[u8]>) -> Option<SillMap> {
        let mut m = SillMap::default();
        if !m.read_feats(feat) {
            return None;
        }
        if !m.read_sill(sill) {
            return None;
        }
        Some(m)
    }

    fn read_feats(&mut self, feat: Option<&[u8]>) -> bool {
        let Some(d) = feat else { return true };
        if d.len() < 12 {
            return false;
        }
        let version = u32_at(d, 0);
        let num = usize::from(u16_at(d, 4));
        if num == 0 {
            return true;
        }
        if version < 0x0001_0000 || 12 + num * 16 > d.len() {
            return false;
        }
        let mut p = 12usize;
        let mut bits: u16 = 0;
        let mut defs = Vec::with_capacity(num);
        for _ in 0..num {
            let label = if version < 0x0002_0000 {
                p += 2;
                u32::from(u16_at(d, p - 2))
            } else {
                p += 4;
                u32_at(d, p - 4)
            };
            let num_settings = usize::from(u16_at(d, p));
            p += 2;
            if version >= 0x0002_0000 {
                p += 2;
            }
            let offset = u32_at(d, p) as usize;
            p += 4;
            let flags = u16_at(d, p);
            let ui_name = u16_at(d, p + 2);
            p += 4;
            if offset > d.len() || offset + num_settings * 4 > d.len() {
                return false;
            }
            let (settings, max_val, def) = if num_settings != 0 {
                let mut s = Vec::with_capacity(num_settings);
                let mut max: u16 = 0;
                for k in 0..num_settings {
                    let v = i16_at(d, offset + 4 * k);
                    s.push((v, u16_at(d, offset + 4 * k + 2)));
                    if v as u16 > max {
                        max = v as u16;
                    }
                }
                let def = s[0].0 as u16;
                (s, u32::from(max), def)
            } else {
                (Vec::new(), 0xffff_ffff, 0)
            };
            defs.push(def);
            self.feats.push(FeatureRef::new(&mut bits, max_val, label, ui_name, flags, settings));
        }
        self.default = Features { vals: alloc::vec![0; usize::from(bits / 32) + 1] };
        for (f, &def) in self.feats.iter().zip(&defs) {
            f.apply(u32::from(def), &mut self.default);
        }
        // (qsort by id; glibc's is a stable merge sort)
        self.named = (0..self.feats.len()).collect();
        let feats = &self.feats;
        self.named.sort_by_key(|&i| feats[i].id);
        true
    }

    fn read_sill(&mut self, sill: Option<&[u8]>) -> bool {
        let Some(d) = sill else { return true };
        if d.len() < 12 || u32_at(d, 0) != 0x0001_0000 {
            return false;
        }
        let n = usize::from(u16_at(d, 4));
        if self.feats.is_empty() {
            return true;
        }
        if d.len() < n * 8 + 12 {
            return false;
        }
        for i in 0..n {
            let e = 12 + 8 * i;
            let langid = u32_at(d, e);
            let num_settings = usize::from(u16_at(d, e + 4));
            let offset = usize::from(u16_at(d, e + 6));
            if offset + 8 * num_settings > d.len() && num_settings > 0 {
                return false;
            }
            let mut feats = self.default.clone();
            for j in 0..num_settings {
                let name = u32_at(d, offset + 8 * j);
                let val = u16_at(d, offset + 8 * j + 4);
                if let Some(r) = self.find(name) {
                    r.apply(u32::from(val), &mut feats);
                }
            }
            if let Some(r) = self.find(1) {
                r.apply(langid, &mut feats);
            }
            self.langs.push((langid, feats));
        }
        true
    }

    /// `findFeatureRef(name)`.
    pub(crate) fn find(&self, id: u32) -> Option<&FeatureRef> {
        self.named.iter().map(|&i| &self.feats[i]).find(|f| f.id == id)
    }

    /// `cloneFeatures(langname)`.
    pub(crate) fn clone_features(&self, lang: u32) -> Features {
        if lang != 0
            && let Some((_, f)) = self.langs.iter().find(|(l, _)| *l == lang)
        {
            return f.clone();
        }
        self.default.clone()
    }

    /// `numLanguages`.
    pub(crate) fn num_languages(&self) -> usize {
        self.langs.len()
    }

    /// `getLangName`.
    pub(crate) fn lang_name(&self, i: usize) -> u32 {
        self.langs.get(i).map_or(0, |l| l.0)
    }

    /// `featureRef(index)`.
    pub(crate) fn feature(&self, i: u8) -> Option<&FeatureRef> {
        self.feats.get(usize::from(i))
    }
}
