// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Face.cpp, src/inc/Face.h,
// src/gr_face.cpp, src/gr_features.cpp, src/gr_segment.cpp),
// LGPL-2.1-or-later.

//! A face (`gr_face`), loaded as `gr_make_face_with_ops(...,
//! gr_face_preloadAll)` loads it, and the C API's face, feature and
//! segment functions.

use alloc::string::String;
use alloc::vec::Vec;

use crate::be::{u16_at, u32_at};
use crate::cmap::Cmap;
use crate::feature_map::{FeatureRef, Features, HIDDEN, SillMap};
use crate::font::Font;
use crate::glyph_cache::{GlyphCache, GlyphTables, metric};
use crate::lz4;
use crate::name_table::NameTable;
use crate::segment::Segment;
use crate::silf::{FaceInfo, Silf};
use crate::ttf;

/// What a `Silf` subtable is read against while the face loads.
pub(crate) struct FaceLoad {
    pub glyphs: GlyphCache,
    pub num_features: u16,
}

/// A Graphite face.
pub struct Face {
    pub(crate) glyphs: GlyphCache,
    pub(crate) cmap: Cmap,
    pub(crate) sill: SillMap,
    pub(crate) silfs: Vec<Silf>,
    names: Option<NameTable>,
}

/// `Face::Table(face, tag, version)`: the table if it passes
/// `CheckTable`, decompressed when its version is at least `version`.
fn table(get: &dyn Fn(u32) -> Option<Vec<u8>>, t: u32, version: u32) -> Option<Vec<u8>> {
    let d = get(t)?;
    if !ttf::check_table(t, &d) {
        return None;
    }
    if u32_at(&d, 0) >= version {
        return decompress(d);
    }
    Some(d)
}

/// `Face::Table::decompress`.
fn decompress(d: Vec<u8>) -> Option<Vec<u8>> {
    if d.len() < 20 {
        return Some(d);
    }
    let version = u32_at(&d, 0);
    let hdr = u32_at(&d, 4);
    match hdr >> 27 {
        0 => Some(d),
        1 => {
            let size = (hdr & 0x07ff_ffff) as usize;
            if size < 4 {
                return None;
            }
            let out = lz4::decompress(&d[8..], size)?;
            if out.len() != size || u32_at(&out, 0) != version {
                return None;
            }
            Some(out)
        }
        _ => None,
    }
}

/// `zeropad(x)`: a tag's trailing spaces made zeros.
fn zeropad(x: u32) -> u32 {
    if x == 0x2020_2020 {
        return 0;
    }
    if x & 0x00FF_FFFF == 0x0020_2020 {
        return x & 0xFF00_0000;
    }
    if x & 0x0000_FFFF == 0x0000_2020 {
        return x & 0xFFFF_0000;
    }
    if x & 0x0000_00FF == 0x0000_0020 {
        return x & 0xFFFF_FF00;
    }
    x
}

impl Face {
    /// `gr_make_face_with_ops(handle, ops, gr_face_preloadAll)`: `get`
    /// answers a table's bytes by tag. `None` where graphite2 fails.
    #[must_use]
    pub fn new(get: &dyn Fn(u32) -> Option<Vec<u8>>) -> Option<Face> {
        let silf = table(get, ttf::SILF, 0x0005_0000)?;
        // readGlyphs
        let t = |tag| table(get, tag, 0xffff_ffff);
        let (head, hhea, hmtx, glyf, loca, maxp) = (
            t(ttf::HEAD),
            t(ttf::HHEA),
            t(ttf::HMTX),
            t(ttf::GLYF),
            t(ttf::LOCA),
            t(ttf::MAXP),
        );
        let glat = table(get, ttf::GLAT, 0x0003_0000);
        let gloc = t(ttf::GLOC);
        let glyphs = GlyphCache::new(&GlyphTables {
            head: head.as_deref(),
            hhea: hhea.as_deref(),
            hmtx: hmtx.as_deref(),
            glyf: glyf.as_deref(),
            loca: loca.as_deref(),
            maxp: maxp.as_deref(),
            glat: glat.as_deref(),
            gloc: gloc.as_deref(),
        });
        if glyphs.num_glyphs() == 0 || glyphs.units_per_em() == 0 {
            return None;
        }
        let cmap = Cmap::new(t(ttf::CMAP).as_deref())?;
        let names = t(ttf::NAME).and_then(|n| NameTable::new(&n));
        // readFeatures
        let sill = SillMap::read(t(ttf::FEAT).as_deref(), t(ttf::SILL).as_deref())?;
        // readGraphite
        let load = FaceLoad {
            glyphs,
            num_features: sill.feats.len() as u16,
        };
        let silfs = read_graphite(&silf, &load)?;
        Some(Face {
            glyphs: load.glyphs,
            cmap,
            sill,
            silfs,
            names,
        })
    }

    /// `getGlyphMetric(gid, metric)`.
    pub(crate) fn glyph_metric(&self, gid: u16, m: u8) -> i32 {
        match m {
            metric::ASCENT | metric::DESCENT => 0,
            _ => {
                if gid >= self.glyphs.num_glyphs() {
                    return 0;
                }
                self.glyphs.glyph(gid).metric(m)
            }
        }
    }

    /// `chooseSilf(script)`.
    pub(crate) fn choose_silf(&self) -> Option<&Silf> {
        self.silfs.first()
    }

    /// `findPseudo(uid)`.
    pub(crate) fn find_pseudo(&self, uid: u32) -> u16 {
        self.silfs.first().map_or(0, |s| s.find_pseudo(uid))
    }

    /// `gr_face_featureval_for_lang(face, langname)`.
    #[must_use]
    pub fn featureval_for_lang(&self, lang: u32) -> Features {
        self.sill.clone_features(zeropad(lang))
    }

    /// `gr_face_find_fref(face, featId)`.
    #[must_use]
    pub fn find_fref(&self, id: u32) -> Option<&FeatureRef> {
        self.sill.find(zeropad(id))
    }

    /// `gr_face_n_fref(face)`: the features not hidden.
    #[must_use]
    pub fn n_fref(&self) -> u16 {
        self.sill
            .feats
            .iter()
            .filter(|f| f.flags() & HIDDEN == 0)
            .count() as u16
    }

    /// `gr_face_fref(face, i)`: the `i`th feature not hidden.
    #[must_use]
    pub fn fref(&self, i: u16) -> Option<&FeatureRef> {
        self.sill
            .feats
            .iter()
            .filter(|f| f.flags() & HIDDEN == 0)
            .nth(usize::from(i))
    }

    /// `gr_face_n_languages`.
    #[must_use]
    pub fn n_languages(&self) -> u16 {
        self.sill.num_languages() as u16
    }

    /// `gr_face_lang_by_index`.
    #[must_use]
    pub fn lang_by_index(&self, i: u16) -> u32 {
        self.sill.lang_name(usize::from(i))
    }

    /// `gr_face_n_glyphs`.
    #[must_use]
    pub fn n_glyphs(&self) -> u16 {
        self.glyphs.num_glyphs()
    }

    /// `gr_face_info(face, script)`.
    #[must_use]
    pub fn info(&self) -> Option<FaceInfo> {
        self.choose_silf().map(|s| s.info)
    }

    /// `gr_face_is_char_supported(face, usv, script)`.
    #[must_use]
    pub fn is_char_supported(&self, usv: u32) -> bool {
        self.cmap.get(usv) != 0 || self.find_pseudo(usv) != 0
    }

    /// `gr_fref_label(fref, &langId, gr_utf8, &len)`: the label and the
    /// language it was found in.
    #[must_use]
    pub fn fref_label(&self, f: &FeatureRef, lang: u16) -> Option<(String, u16)> {
        self.names.as_ref()?.get_name(lang, f.name_id())
    }

    /// `gr_fref_value_label(fref, setting, &langId, gr_utf8, &len)`.
    #[must_use]
    pub fn fref_value_label(
        &self,
        f: &FeatureRef,
        setting: u16,
        lang: u16,
    ) -> Option<(String, u16)> {
        let &(_, label) = f.settings.get(usize::from(setting))?;
        self.names.as_ref()?.get_name(lang, label)
    }

    /// `gr_make_seg(font, face, script, feats, gr_utf32, chars, n, dir)`:
    /// `None` where graphite2 returns no segment.
    #[must_use]
    pub fn make_seg(
        &self,
        font: Option<&Font>,
        feats: Option<&Features>,
        chars: &[u32],
        dir: i32,
    ) -> Option<Segment<'_>> {
        let tmp;
        let feats = match feats {
            Some(f) => f,
            None => {
                tmp = self.sill.clone_features(0);
                &tmp
            }
        };
        let silf = self.choose_silf()?;
        let mut seg = Segment::new(chars.len(), self, silf, dir);
        // read_text (gr_utf32)
        seg.feats.push(feats.clone());
        let fid = (seg.feats.len() - 1) as i32;
        for (i, &c) in chars.iter().enumerate() {
            let usv = if c < 0x11_0000 { c } else { 0xFFFD };
            let mut gid = self.cmap.get(usv);
            if gid == 0 {
                gid = self.find_pseudo(usv);
            }
            seg.append_slot(i, usv as i32, gid, fid, i);
        }
        if !self.run_graphite(&mut seg, silf) {
            return None;
        }
        seg.finalise(font, true);
        Some(seg)
    }

    /// `Face::runGraphite(seg, silf)`.
    fn run_graphite(&self, seg: &mut Segment, silf: &Silf) -> bool {
        if seg.dir & 3 == 3 && silf.b_pass == 0xFF {
            seg.do_mirror(u16::from(silf.a_mirror));
        }
        let mut res = silf.run_graphite(seg, 0, silf.p_pass, true);
        if res {
            let n = seg.charinfo.len();
            seg.associate_chars(0, n);
            if silf.flags & 0x20 != 0 {
                res &= seg.init_collisions();
            }
            if res {
                res &= silf.run_graphite(seg, silf.p_pass, silf.num_passes, false);
            }
        }
        res
    }
}

impl FeatureRef {
    /// `gr_fref_id`.
    #[must_use]
    pub fn fref_id(&self) -> u32 {
        self.id()
    }

    /// `gr_fref_n_values`.
    #[must_use]
    pub fn n_values(&self) -> u16 {
        self.settings.len() as u16
    }

    /// `gr_fref_value(fref, settingno)`.
    #[must_use]
    pub fn setting_value(&self, i: u16) -> i16 {
        self.settings.get(usize::from(i)).map_or(0, |s| s.0)
    }

    /// `gr_fref_feature_value(fref, feats)`.
    #[must_use]
    pub fn feature_value(&self, feats: &Features) -> u16 {
        self.value(feats) as u16
    }

    /// `gr_fref_set_feature_value(fref, val, feats)`.
    pub fn set_feature_value(&self, val: u16, feats: &mut Features) -> bool {
        self.apply(u32::from(val), feats)
    }
}

/// `Face::readGraphite(silf)`.
fn read_graphite(d: &[u8], face: &FaceLoad) -> Option<Vec<Silf>> {
    if d.len() < 20 {
        return None;
    }
    let version = u32_at(d, 0);
    if version < 0x0002_0000 {
        return None;
    }
    let mut p = 4;
    if version >= 0x0003_0000 {
        p += 4;
    }
    let n = usize::from(u16_at(d, p));
    p += 4;
    let mut silfs = Vec::with_capacity(n);
    let mut have_passes = false;
    for i in 0..n {
        let offset = u32_at(d, p) as usize;
        p += 4;
        let next = if i == n - 1 {
            d.len()
        } else {
            u32_at(d, p) as usize
        };
        if next > d.len() || offset >= next {
            return None;
        }
        let s = Silf::read(&d[offset..next], face, version)?;
        if s.num_passes != 0 {
            have_passes = true;
        }
        silfs.push(s);
    }
    have_passes.then_some(silfs)
}
