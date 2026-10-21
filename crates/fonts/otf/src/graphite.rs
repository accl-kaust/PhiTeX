//! Graphite shaping as XeTeX gets it for a `/GR` font: HarfBuzz's
//! `graphite2` shaper (`hb-graphite2.cc`, HarfBuzz 14.5.0) over
//! [`phitex_graphite`] (graphite2 1.3.15).
//!
//! The buffer as `hb_buffer_add_utf16` makes it (one item a code point,
//! its cluster the index of its first UTF-16 unit, a lone surrogate
//! U+FFFD), reversed where the direction is against the script's; the
//! segment made in design units (no `gr_font`: XeTeX's HarfBuzz font is
//! scaled to its units per em, so `xscale` and `yscale` are 1); then
//! `_hb_graphite2_shape`'s clusters and positions, its `float`, `double`
//! and saturating `int` arithmetic kept.

#![allow(clippy::if_not_else, clippy::struct_field_names)]

use alloc::string::String;
use alloc::vec::Vec;

use crate::Tag;
use crate::face::Face;
use crate::shape::{Feature, ShapedGlyph};

/// `hb_language_from_string`'s canonical form: lower case, `_` as `-`,
/// cut at the first other character; `None` for an empty string.
#[must_use]
pub fn language_from_string(s: &[u8]) -> Option<String> {
    let mut out = String::new();
    for &c in s {
        let m = match c {
            b'-' | b'_' => b'-',
            b'0'..=b'9' | b'a'..=b'z' => c,
            b'A'..=b'Z' => c.to_ascii_lowercase(),
            _ => break,
        };
        out.push(char::from(m));
    }
    (!s.is_empty() && s[0] != 0).then_some(out)
}

/// `hb_script_get_horizontal_direction`: `Some(true)` right to left,
/// `Some(false)` left to right, `None` invalid.
fn horizontal_rtl(script: Option<Tag>) -> Option<bool> {
    let Some(s) = script else { return Some(false) };
    if crate::xetex::script_is_rtl(s) {
        return Some(true);
    }
    if [b"Hung", b"Ital", b"Runr", b"Tfng"].iter().any(|t| crate::tag(t) == s) {
        return None;
    }
    Some(false)
}

/// `hb_tag_from_string(s, len)` (space padded).
fn tag_from(s: &[u8]) -> u32 {
    let mut t = [b' '; 4];
    for (i, &c) in s.iter().take(4).enumerate() {
        if c == 0 {
            break;
        }
        t[i] = c;
    }
    u32::from_be_bytes(t)
}

/// `hb_clamp_to<hb_position_t>(double)`.
fn clamp(v: f64) -> i32 {
    let lo = f64::from(i32::MIN);
    let hi = f64::from(i32::MAX);
    let v = if v >= lo { v } else { lo };
    let v = if v <= hi { v } else { hi };
    v as i32
}

#[derive(Clone, Copy, Default)]
struct Cluster {
    base_char: u32,
    num_chars: u32,
    base_glyph: u32,
    num_glyphs: u32,
    cluster: u32,
    advance: i32,
}

/// One buffer item: (code point or glyph, cluster).
type Item = (u32, u32);

/// `hb_buffer_t::merge_clusters(start, end)` (no out-buffer).
fn merge_clusters(info: &mut [Item], start: usize, end: usize) {
    if end < start + 2 {
        return;
    }
    let (mut start, mut end) = (start, end);
    let cluster = info[start..end].iter().map(|i| i.1).min().unwrap_or(0);
    if cluster != info[end - 1].1 {
        while end < info.len() && info[end - 1].1 == info[end].1 {
            end += 1;
        }
    }
    if cluster != info[start].1 {
        while start > 0 && info[start - 1].1 == info[start].1 {
            start -= 1;
        }
    }
    for i in &mut info[start..end] {
        i.1 = cluster;
    }
}

/// `hb_buffer_reverse_clusters`: the clusters' order reversed, each
/// cluster's own order kept (with the positions, if any).
fn reverse_clusters<T>(info: &mut [Item], pos: &mut [T]) {
    let n = info.len();
    if n == 0 {
        return;
    }
    let mut start = 0;
    for i in 1..n {
        if info[i - 1].1 != info[i].1 {
            info[start..i].reverse();
            if !pos.is_empty() {
                pos[start..i].reverse();
            }
            start = i;
        }
    }
    info[start..n].reverse();
    if !pos.is_empty() {
        pos[start..n].reverse();
    }
    info.reverse();
    if !pos.is_empty() {
        pos.reverse();
    }
}

/// The run `text[start..start + len]` shaped, as `hb_shape_plan_execute`
/// with the `graphite2` shaper gives it; `None` where the shaper fails
/// (graphite2 makes no segment).
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn shape(
    face: &Face,
    gr: &phitex_graphite::Face,
    text: &[u16],
    start: usize,
    len: usize,
    rtl: bool,
    vertical: bool,
    script: Option<Tag>,
    language: Option<&str>,
    features: &[Feature],
) -> Option<Vec<ShapedGlyph>> {
    // hb_buffer_add_utf16
    let mut info: Vec<Item> = Vec::with_capacity(len);
    let end = start + len;
    let mut i = start;
    while i < end {
        let u = u32::from(text[i]);
        let cl = i as u32;
        if (0xD800..0xDC00).contains(&u) && i + 1 < end && (0xDC00..0xE000).contains(&u32::from(text[i + 1])) {
            let l = u32::from(text[i + 1]);
            info.push((0x10000 + ((u - 0xD800) << 10) + (l - 0xDC00), cl));
            i += 2;
        } else {
            info.push((if (0xD800..0xE000).contains(&u) { 0xFFFD } else { u }, cl));
            i += 1;
        }
    }
    if info.is_empty() {
        return Some(Vec::new());
    }
    // the features: the language's, then each requested one
    let lang = language.unwrap_or("c");
    let lang_part = lang.split('-').next().unwrap_or("");
    let mut feats = gr.featureval_for_lang(tag_from(lang_part.as_bytes()));
    for f in features {
        if let Some(r) = gr.find_fref(f.tag) {
            r.set_feature_value(f.value as u16, &mut feats);
        }
    }
    // the direction against the script's
    let mut backward = rtl && !vertical;
    let horiz = horizontal_rtl(script);
    let reverse = if vertical {
        false
    } else {
        horiz.is_some_and(|h| h != rtl)
    };
    if reverse {
        reverse_clusters::<()>(&mut info, &mut []);
        backward = !backward;
    }
    let chars: Vec<u32> = info.iter().map(|i| i.0).collect();
    let seg = gr.make_seg(None, Some(&feats), &chars, 2 | i32::from(backward))?;
    let glyph_count = seg.n_slots();
    if glyph_count == 0 {
        return Some(Vec::new());
    }
    let n = info.len();
    let mut clusters = alloc::vec![Cluster::default(); n];
    let mut glyphs: Vec<Item> = Vec::with_capacity(glyph_count);
    clusters[0].cluster = info[0].1;
    // (the font's scale is its units per em: `hb_font_set_scale(upem,
    // upem)`)
    let (x_scale, y_scale) = (i32::from(face.units_per_em()), i32::from(face.units_per_em()));
    let upem = f32::from(face.units_per_em());
    let xscale = x_scale as f32 / upem;
    let mut yscale = y_scale as f32 / upem;
    yscale *= yscale / xscale;
    let mut curradv: i32 = 0;
    if backward {
        curradv = clamp(f64::from(seg.slot_origin_x(seg.first_slot()) * xscale));
        clusters[0].advance = clamp(f64::from(seg.advance_x() * xscale) - f64::from(curradv));
    }
    let mut ci = 0usize;
    for (ic, is) in seg.slots().enumerate() {
        let before = seg.slot_before(is);
        let after = seg.slot_after(is);
        if before < 0 || after < before || after as usize >= n {
            return None;
        }
        let (before, after) = (before as u32, after as u32);
        glyphs.push((u32::from(seg.slot_gid(is)), info[before as usize].1.min(info[after as usize].1)));
        while clusters[ci].base_char > before && ci != 0 {
            let c = clusters[ci];
            let p = &mut clusters[ci - 1];
            p.num_chars += c.num_chars;
            p.num_glyphs += c.num_glyphs;
            p.advance = p.advance.saturating_add(c.advance);
            ci -= 1;
        }
        if seg.slot_can_insert_before(is)
            && clusters[ci].num_chars != 0
            && before >= clusters[ci].base_char + clusters[ci].num_chars
        {
            let base_char = clusters[ci].base_char + clusters[ci].num_chars;
            let mut c = Cluster {
                base_char,
                cluster: info[base_char as usize].1,
                num_chars: before - base_char,
                base_glyph: ic as u32,
                num_glyphs: 0,
                advance: 0,
            };
            let ox = seg.slot_origin_x(is) * xscale;
            if backward {
                c.advance = clamp(f64::from(curradv) - f64::from(ox));
                curradv = curradv.saturating_sub(c.advance);
            } else {
                let origin_x = clamp(f64::from(ox));
                clusters[ci].advance = clusters[ci].advance.saturating_add(origin_x.saturating_sub(curradv));
                curradv = origin_x;
            }
            clusters[ci + 1] = c;
            ci += 1;
        }
        clusters[ci].num_glyphs += 1;
        if clusters[ci].base_char + clusters[ci].num_chars < after + 1 {
            clusters[ci].num_chars = after + 1 - clusters[ci].base_char;
        }
    }
    if backward {
        clusters[ci].advance = clusters[ci].advance.saturating_add(curradv);
    } else {
        clusters[ci].advance =
            clamp(f64::from(clusters[ci].advance) + f64::from(seg.advance_x() * xscale) - f64::from(curradv));
    }
    ci += 1;
    // the glyphs, each with its graphite cluster's advance and index
    let mut out: Vec<Item> = alloc::vec![(0, 0); glyph_count];
    let mut var1 = alloc::vec![0i32; glyph_count];
    let mut var2 = alloc::vec![0u32; glyph_count];
    for (i, c) in clusters[..ci].iter().enumerate() {
        for j in 0..c.num_glyphs {
            let k = (c.base_glyph + j) as usize;
            out[k] = glyphs[k];
            var1[k] = c.advance;
            var2[k] = i as u32;
        }
    }
    for c in &clusters[..ci] {
        merge_clusters(&mut out, c.base_glyph as usize, (c.base_glyph + c.num_glyphs) as usize);
    }
    // positions: (x_advance, y_advance, x_offset, y_offset)
    let mut pos: Vec<(i32, i32, i32, i32)> = alloc::vec![(0, 0, 0, 0); glyph_count];
    let mut curr_gr_cluster = u32::MAX;
    let mut curradvy: i32 = 0;
    if !backward {
        let mut curradvx: i32 = 0;
        for (k, is) in seg.slots().enumerate() {
            let p = &mut pos[k];
            p.2 = clamp(f64::from(seg.slot_origin_x(is) * xscale) - f64::from(curradvx));
            p.3 = clamp(f64::from(seg.slot_origin_y(is) * yscale) - f64::from(curradvy));
            if var2[k] != curr_gr_cluster {
                p.0 = var1[k];
                curradvx = curradvx.saturating_add(p.0);
                curr_gr_cluster = var2[k];
            } else {
                p.0 = 0;
            }
            p.1 = clamp(f64::from(seg.slot_advance_y(is, None) * yscale));
            curradvy = curradvy.saturating_add(p.1);
        }
    } else {
        let mut curradvx = clamp(f64::from(seg.advance_x() * xscale));
        for (k, is) in seg.slots().enumerate() {
            let p = &mut pos[k];
            if var2[k] != curr_gr_cluster {
                p.0 = var1[k];
                curradvx = curradvx.saturating_sub(p.0);
                curr_gr_cluster = var2[k];
            } else {
                p.0 = 0;
            }
            p.1 = clamp(f64::from(seg.slot_advance_y(is, None) * yscale));
            curradvy = curradvy.saturating_sub(p.1);
            p.2 = clamp(
                f64::from(seg.slot_origin_x(is) * xscale) - f64::from(var1[k]) - f64::from(curradvx) + f64::from(p.0),
            );
            p.3 = clamp(f64::from(seg.slot_origin_y(is) * yscale) - f64::from(curradvy));
        }
        reverse_clusters(&mut out, &mut pos);
    }
    Some(
        out.iter()
            .zip(&pos)
            .map(|(&(gid, cluster), &(xa, ya, xo, yo))| ShapedGlyph {
                gid,
                cluster,
                x_advance: xa,
                y_advance: ya,
                x_offset: xo,
                y_offset: yo,
            })
            .collect(),
    )
}

/// `findGraphiteFeatureNamed(engine, name, namelength)`: the id of the
/// first feature (not hidden) whose English label starts with `name`.
#[must_use]
pub fn feature_named(gr: &phitex_graphite::Face, name: &[u8]) -> Option<u32> {
    for i in 0..gr.n_fref() {
        let f = gr.fref(i)?;
        if let Some((label, _)) = gr.fref_label(f, 0x409)
            && strncmp_prefix(label.as_bytes(), name)
        {
            return Some(f.fref_id());
        }
    }
    None
}

/// `findGraphiteFeatureSettingNamed(engine, id, name, namelength)`.
#[must_use]
pub fn setting_named(gr: &phitex_graphite::Face, id: u32, name: &[u8]) -> Option<i32> {
    let f = gr.find_fref(id)?;
    for i in 0..f.n_values() {
        if let Some((label, _)) = gr.fref_value_label(f, i, 0x409)
            && strncmp_prefix(label.as_bytes(), name)
        {
            return Some(i32::from(f.setting_value(i)));
        }
    }
    None
}

/// `strncmp(label, name, len(name)) == 0`.
fn strncmp_prefix(label: &[u8], name: &[u8]) -> bool {
    for (k, &c) in name.iter().enumerate() {
        let l = label.get(k).copied().unwrap_or(0);
        if l != c {
            return false;
        }
        if c == 0 {
            return true;
        }
    }
    true
}
