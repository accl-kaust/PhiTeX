//! XeTeX's font-name matching (`XeTeXFontMgr.cpp`, `XeTeXFontMgr_FC.cpp`)
//! over a [`FontIndex`] in fontconfig's list order.
//!
//! Like XeTeX's, the manager is stateful: its maps fill as names are
//! looked up (`searchForHostPlatformFonts` caches the faces whose
//! fontconfig names match, then everything once a name is not found), and
//! which face a later name finds can depend on what was cached before
//! (the first face cached with a full name, a PostScript name or a style
//! keeps it). One manager per job, as XeTeX has one per run.
//!
//! fontconfig's own names for a face (`FC_FAMILY`, `FC_STYLE`,
//! `FC_FULLNAME`, which only decide what gets cached when) are derived
//! from the raw records ([`fc_names`]).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::index::{FaceEntry, FontIndex, LOADABLE, SFNT};
use crate::names::{self, NameRecord};
use crate::xetex::Diagnostic;

#[derive(Clone, Copy, Debug, Default)]
struct OpSize {
    design_size: f64,
    min_size: f64,
    max_size: f64,
    sub_family_id: u32,
    name_code: u32,
}

#[derive(Clone, Debug)]
struct Font {
    entry: usize,
    full_name: Option<String>,
    ps_name: String,
    parent: Option<usize>,
    op_size: OpSize,
    weight: u16,
    width: u16,
    slant: i16,
    is_reg: bool,
    is_bold: bool,
    is_italic: bool,
}

#[derive(Clone, Debug, Default)]
struct Family {
    styles: BTreeMap<String, usize>,
    min_weight: u16,
    max_weight: u16,
    min_width: u16,
    max_width: u16,
    min_slant: i16,
    max_slant: i16,
}

/// `NameCollection`.
#[derive(Clone, Debug, Default)]
struct NameCollection {
    family_names: Vec<String>,
    style_names: Vec<String>,
    full_names: Vec<String>,
    ps_name: String,
}

/// fontconfig's family, style and full names of a face.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FcNames {
    pub family: Vec<String>,
    pub style: Vec<String>,
    pub fullname: Vec<String>,
}

/// fontconfig's `FcStrCmpIgnoreBlanksAndCase(a, b) == 0`.
fn fc_same(a: &str, b: &str) -> bool {
    let fold = |s: &str| -> Vec<char> {
        s.chars()
            .filter(|&c| c != ' ')
            .flat_map(char::to_lowercase)
            .collect()
    };
    fold(a) == fold(b)
}

/// `FcSfntNameTranscode` (without iconv: UTF-16, Mac Roman, ASCII and
/// Latin-1 names; `None` for the others or broken UTF-16).
fn fc_transcode(r: &NameRecord) -> Option<String> {
    let utf16 = |b: &[u8]| -> Option<String> {
        if b.len() % 2 != 0 {
            return None;
        }
        let units = b.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]]));
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .ok()
    };
    match (r.platform, r.encoding) {
        (0, _) | (3, 0 | 1 | 10) | (2, 1) => utf16(&r.bytes),
        (1, 0) if r.language < 0x100 && r.language != 11 => Some(names::decode_mac_roman(&r.bytes)),
        (2, 0 | 2) => Some(r.bytes.iter().map(|&c| char::from(c)).collect()),
        _ => None,
    }
}

/// Whether fontconfig gives a record the language "en".
fn fc_is_en(r: &NameRecord) -> bool {
    match r.platform {
        1 => r.language == 0,
        3 => r.language & 0x3FF == 0x09,
        _ => false,
    }
}

/// The names fontconfig lists for a face (`FcFreeTypeQueryFaceInternal`):
/// by platform (Microsoft, Unicode, Macintosh, ISO), then name id (WWS,
/// typographic, legacy family; Mac and plain full name; WWS, typographic,
/// legacy subfamily), then encoding, English first, language; trimmed, and
/// each once ignoring case and blanks. A face without a style gets
/// "Regular"; one without a full name, its first English family and style.
#[must_use]
pub fn fc_names(entry: &FaceEntry) -> FcNames {
    let records = &entry.names;
    // A variable font itself (fontconfig's id 0x8000…, listed at index 0):
    // no style or full name. A named instance: its own subfamily, and no
    // other style or full name.
    let variable = entry.flags & crate::index::VARIABLE != 0 && entry.index >> 16 == 0;
    let instance = entry.instance_subfamily;
    let mut sorted: Vec<(usize, &NameRecord)> = records.iter().enumerate().collect();
    sorted.sort_by(|(ia, a), (ib, b)| {
        (a.platform, a.name_id, a.encoding)
            .cmp(&(b.platform, b.name_id, b.encoding))
            .then_with(|| {
                if a.language == b.language {
                    core::cmp::Ordering::Equal
                } else {
                    let ea = (a.platform == 1 && a.language == 0)
                        || (a.platform == 3 && a.language == 0x409);
                    let eb = (b.platform == 1 && b.language == 0)
                        || (b.platform == 3 && b.language == 0x409);
                    if ea {
                        core::cmp::Ordering::Less
                    } else if eb {
                        core::cmp::Ordering::Greater
                    } else {
                        a.language.cmp(&b.language)
                    }
                }
            })
            .then(ia.cmp(ib))
    });
    let mut out = FcNames::default();
    let (mut family_en, mut style_en) = (None, None);
    for platform in [3u16, 0, 1, 2] {
        for id in [21u16, 16, 1, 18, 4, 22, 17, 2] {
            if instance.is_some() && matches!(id, 22 | 17 | 4) {
                continue;
            }
            if variable && !matches!(id, 21 | 16 | 1) {
                continue;
            }
            let lookup = match (instance, id) {
                (Some(strid), 2) => strid,
                _ => id,
            };
            for &(_, r) in sorted
                .iter()
                .filter(|(_, r)| r.platform == platform && r.name_id == lookup)
            {
                let Some(s) = fc_transcode(r) else { continue };
                let s = s
                    .trim_start_matches(' ')
                    .trim_end_matches([' ', '\r', '\n'])
                    .to_string();
                let (list, en) = match id {
                    21 | 16 | 1 => (&mut out.family, &mut family_en),
                    18 | 4 => (&mut out.fullname, &mut style_en),
                    _ => (&mut out.style, &mut style_en),
                };
                if list.iter().any(|x| fc_same(x, &s)) {
                    continue;
                }
                if id != 18 && id != 4 && en.is_none() && fc_is_en(r) {
                    *en = Some(list.len());
                }
                list.push(s);
            }
        }
    }
    if variable {
        return out;
    }
    if out.style.is_empty() {
        out.style.push(String::from("Regular"));
        style_en = Some(0);
    }
    if out.fullname.is_empty() && !out.family.is_empty() {
        let fam = out.family[family_en.unwrap_or(0)].trim_end().to_string();
        let sty = out.style[style_en.unwrap_or(0)].trim_start().to_string();
        out.fullname.push(alloc::format!("{fam} {sty}"));
    }
    out
}

/// What [`FontManager::find_font`] found.
#[derive(Clone, Debug)]
pub struct Found {
    /// The font (its index entry: [`FontManager::entry`]).
    pub font: usize,
    /// `loadedfontdesignsize` after the lookup.
    pub design_size: i32,
    /// What `\tracingfonts` prints.
    pub diagnostics: Vec<Diagnostic>,
}

/// XeTeX's font manager over an index.
pub struct FontManager {
    index: Arc<FontIndex>,
    fc: Vec<Option<FcNames>>,
    fonts: Vec<Font>,
    families: Vec<Family>,
    name_to_font: BTreeMap<String, usize>,
    name_to_family: BTreeMap<String, usize>,
    entry_to_font: BTreeMap<usize, usize>,
    ps_name_to_font: BTreeMap<String, usize>,
    cached_all: bool,
    req_engine: u8,
    /// `\tracingfonts` (for the `-> path` line).
    pub tracing_fonts: i32,
}

/// `strncmp(cp, s, n) == 0`.
fn starts(s: &[u8], i: usize, p: &[u8]) -> bool {
    s.get(i..i + p.len()) == Some(p)
}

impl FontManager {
    /// A manager with nothing cached (`XeTeXFontMgr_FC::initialize`).
    #[must_use]
    pub fn new(index: Arc<FontIndex>) -> Self {
        let n = index.entries.len();
        FontManager {
            index,
            fc: alloc::vec![None; n],
            fonts: Vec::new(),
            families: Vec::new(),
            name_to_font: BTreeMap::new(),
            name_to_family: BTreeMap::new(),
            entry_to_font: BTreeMap::new(),
            ps_name_to_font: BTreeMap::new(),
            cached_all: false,
            req_engine: 0,
            tracing_fonts: 0,
        }
    }

    /// The index entry of a font [`FontManager::find_font`] returned.
    #[must_use]
    pub fn entry(&self, font: usize) -> &FaceEntry {
        &self.index.entries[self.fonts[font].entry]
    }

    /// `getReqEngine`: the engine the last lookup's variant asked for
    /// (`b'A'`, `b'O'`, `b'G'` or 0).
    #[must_use]
    pub fn req_engine(&self) -> u8 {
        self.req_engine
    }

    /// `getFullName`.
    #[must_use]
    pub fn full_name(&self, font: usize) -> String {
        let f = &self.fonts[font];
        f.full_name.clone().unwrap_or_else(|| f.ps_name.clone())
    }

    fn fc_names(&mut self, e: usize) -> &FcNames {
        if self.fc[e].is_none() {
            self.fc[e] = Some(fc_names(&self.index.entries[e]));
        }
        self.fc[e].as_ref().expect("set above")
    }

    /// `readNames`.
    fn read_names(&self, e: usize) -> NameCollection {
        let entry = &self.index.entries[e];
        let mut names = NameCollection::default();
        if entry.flags & LOADABLE == 0 {
            return names;
        }
        let Some(ps) = names::postscript_name(&entry.names) else {
            return names;
        };
        names.ps_name = ps;
        if entry.flags & SFNT != 0 {
            let mut family_names: Vec<String> = Vec::new();
            let mut sub_family_names: Vec<String> = Vec::new();
            for r in &entry.names {
                let list: &mut Vec<String> = match r.name_id {
                    names::FULL_NAME => &mut names.full_names,
                    names::FAMILY => &mut names.family_names,
                    names::SUBFAMILY => &mut names.style_names,
                    names::TYPOGRAPHIC_FAMILY => &mut family_names,
                    names::TYPOGRAPHIC_SUBFAMILY => &mut sub_family_names,
                    _ => continue,
                };
                let Some((mut s, preferred)) = names::xetex_decode(r) else {
                    continue;
                };
                // A C string: up to its first NUL.
                if let Some(z) = s.find('\0') {
                    s.truncate(z);
                }
                if preferred {
                    if let Some(i) = list.iter().position(|x| *x == s) {
                        list.remove(i);
                    }
                    list.insert(0, s);
                } else if !list.contains(&s) {
                    list.push(s);
                }
            }
            if !family_names.is_empty() {
                names.family_names = family_names;
            }
            if !sub_family_names.is_empty() {
                names.style_names = sub_family_names;
            }
        }
        names
    }

    /// `getOpSizeRecAndStyleFlags` (from the index entry).
    fn style_flags(&self, font: &mut Font) {
        let e = &self.index.entries[font.entry];
        if e.flags & LOADABLE != 0 {
            if let Some(p) = e.size {
                font.op_size.design_size = f64::from(p.design_size) * 72.27 / 72.0 / 10.0;
                let (min, max) = (
                    f64::from(p.range_start) * 72.27 / 72.0 / 10.0,
                    f64::from(p.range_end) * 72.27 / 72.0 / 10.0,
                );
                if !(p.subfamily_id == 0 && p.subfamily_name_id == 0 && min == 0.0 && max == 0.0) {
                    font.op_size.sub_family_id = u32::from(p.subfamily_id);
                    font.op_size.name_code = u32::from(p.subfamily_name_id);
                    font.op_size.min_size = min;
                    font.op_size.max_size = max;
                }
            }
            if let Some((w, wd, sel)) = e.os2 {
                font.weight = w;
                font.width = wd;
                font.is_reg = sel & (1 << 6) != 0;
                font.is_bold = sel & (1 << 5) != 0;
                font.is_italic = sel & 1 != 0;
            }
            if let Some(ms) = e.mac_style {
                if ms & 1 != 0 {
                    font.is_bold = true;
                }
                if ms & 2 != 0 {
                    font.is_italic = true;
                }
            }
            let angle = super::fix2d(-e.italic_angle);
            font.slant = (1000.0 * libm::tan(angle * core::f64::consts::PI / 180.0)) as i32 as i16;
        }
        if font.weight == 0
            && font.width == 0
            && let Some((w, wd, sl)) = e.fc_style
        {
            font.weight = w;
            font.width = wd;
            font.slant = sl as i16;
        }
    }

    /// `addToMaps`.
    fn add_to_maps(&mut self, e: usize, names: &NameCollection) {
        if self.entry_to_font.contains_key(&e) {
            return;
        }
        if names.ps_name.is_empty() {
            return;
        }
        if self.ps_name_to_font.contains_key(&names.ps_name) {
            return;
        }
        let mut font = Font {
            entry: e,
            full_name: names.full_names.first().cloned(),
            ps_name: names.ps_name.clone(),
            parent: None,
            op_size: OpSize {
                design_size: 10.0,
                ..OpSize::default()
            },
            weight: 0,
            width: 0,
            slant: 0,
            is_reg: false,
            is_bold: false,
            is_italic: false,
        };
        self.style_flags(&mut font);
        let id = self.fonts.len();
        self.ps_name_to_font.insert(names.ps_name.clone(), id);
        self.entry_to_font.insert(e, id);
        for fam_name in &names.family_names {
            let fam = if let Some(&f) = self.name_to_family.get(fam_name) {
                let fam = &mut self.families[f];
                fam.min_weight = fam.min_weight.min(font.weight);
                fam.max_weight = fam.max_weight.max(font.weight);
                fam.min_width = fam.min_width.min(font.width);
                fam.max_width = fam.max_width.max(font.width);
                fam.min_slant = fam.min_slant.min(font.slant);
                fam.max_slant = fam.max_slant.max(font.slant);
                f
            } else {
                let f = self.families.len();
                self.families.push(Family {
                    styles: BTreeMap::new(),
                    min_weight: font.weight,
                    max_weight: font.weight,
                    min_width: font.width,
                    max_width: font.width,
                    min_slant: font.slant,
                    max_slant: font.slant,
                });
                self.name_to_family.insert(fam_name.clone(), f);
                f
            };
            if font.parent.is_none() {
                font.parent = Some(fam);
            }
            for style in &names.style_names {
                self.families[fam].styles.entry(style.clone()).or_insert(id);
            }
        }
        for full in &names.full_names {
            self.name_to_font.entry(full.clone()).or_insert(id);
        }
        self.fonts.push(font);
    }

    fn cache(&mut self, e: usize) -> NameCollection {
        let names = self.read_names(e);
        self.add_to_maps(e, &names);
        names
    }

    /// `cacheFamilyMembers`.
    fn cache_family_members(&mut self, family_names: &[String]) {
        if family_names.is_empty() {
            return;
        }
        for e in 0..self.index.entries.len() {
            if self.entry_to_font.contains_key(&e) {
                continue;
            }
            let hit = self
                .fc_names(e)
                .family
                .iter()
                .any(|s| family_names.iter().any(|j| j == s));
            if hit {
                self.cache(e);
            }
        }
    }

    /// `searchForHostPlatformFonts`.
    fn search(&mut self, name: &str) {
        if self.cached_all {
            return;
        }
        let hyph = name.find('-').filter(|&h| h > 0 && h < name.len() - 1);
        let fam_name = hyph.map(|h| &name[..h]);
        let mut found = false;
        loop {
            for e in 0..self.index.entries.len() {
                if self.entry_to_font.contains_key(&e) {
                    continue;
                }
                if self.cached_all {
                    self.cache(e);
                    continue;
                }
                let fc = self.fc_names(e).clone();
                let mut hit = fc.fullname.iter().any(|s| s == name);
                if !hit {
                    'fam: for s in &fc.family {
                        if s == name || fam_name == Some(s.as_str()) {
                            hit = true;
                            break;
                        }
                        for t in &fc.style {
                            let mut full = s.clone();
                            full.push(' ');
                            full.push_str(t);
                            if full == name {
                                hit = true;
                                break 'fam;
                            }
                        }
                    }
                }
                if hit {
                    let names = self.cache(e);
                    self.cache_family_members(&names.family_names);
                    found = true;
                }
            }
            if found || self.cached_all {
                break;
            }
            self.cached_all = true;
        }
    }

    fn weight_and_width_diff(a: &Font, b: &Font) -> i32 {
        if a.weight == 0 && a.width == 0 {
            return if a.is_bold == b.is_bold { 0 } else { 10000 };
        }
        let mut wid = (i32::from(a.width) - i32::from(b.width)).abs();
        if wid < 10 {
            wid *= 50;
        }
        (i32::from(a.weight) - i32::from(b.weight)).abs() + wid
    }

    fn style_diff(a: &Font, wt: i32, wd: i32, slant: i32) -> i32 {
        let mut wid = (i32::from(a.width) - wd).abs();
        if wid < 10 {
            wid *= 200;
        }
        ((i32::from(a.slant)).abs() - slant.abs()).abs() * 2
            + (i32::from(a.weight) - wt).abs()
            + wid
    }

    fn best_match(&self, fam: usize, wt: i32, wd: i32, slant: i32) -> Option<usize> {
        let mut best: Option<usize> = None;
        for &f in self.families[fam].styles.values() {
            if best.is_none_or(|b| {
                Self::style_diff(&self.fonts[f], wt, wd, slant)
                    < Self::style_diff(&self.fonts[b], wt, wd, slant)
            }) {
                best = Some(f);
            }
        }
        best
    }

    /// `XeTeXFontMgr::findFont(name, variant, ptSize)`: the font for
    /// `name`, with `variant` (`/B/I/S=…/OT…`) applied and edited in place
    /// as XeTeX does (B, I and S removed); `pt_size` in TeX points
    /// (negative: a scale factor).
    pub fn find_font(
        &mut self,
        name: &str,
        variant: Option<&mut String>,
        pt_size: f64,
    ) -> Option<Found> {
        let mut pt_size = pt_size;
        let mut font: Option<usize> = None;
        let mut dsize = 10.0f64;
        let mut design_size: i32 = 655_360;
        for pass in 0..2 {
            if let Some(&f) = self.name_to_font.get(name) {
                font = Some(f);
                if self.fonts[f].op_size.design_size != 0.0 {
                    dsize = self.fonts[f].op_size.design_size;
                }
                break;
            }
            if let Some(h) = name.find('-').filter(|&h| h > 0 && h < name.len() - 1)
                && let Some(&fam) = self.name_to_family.get(&name[..h])
                && let Some(&f) = self.families[fam].styles.get(&name[h + 1..])
            {
                font = Some(f);
                if self.fonts[f].op_size.design_size != 0.0 {
                    dsize = self.fonts[f].op_size.design_size;
                }
                break;
            }
            if let Some(&f) = self.ps_name_to_font.get(name) {
                font = Some(f);
                if self.fonts[f].op_size.design_size != 0.0 {
                    dsize = self.fonts[f].op_size.design_size;
                }
                break;
            }
            if let Some(&fam) = self.name_to_family.get(name) {
                let mut reg_fonts = 0;
                for &f in self.families[fam].styles.values() {
                    if self.fonts[f].is_reg {
                        if reg_fonts == 0 {
                            font = Some(f);
                        }
                        reg_fonts += 1;
                    }
                }
                if font.is_none() || reg_fonts > 1 {
                    let styles = &self.families[fam].styles;
                    if let Some(&f) = styles
                        .get("Regular")
                        .or_else(|| styles.get("Plain"))
                        .or_else(|| styles.get("Normal"))
                        .or_else(|| styles.get("Roman"))
                    {
                        font = Some(f);
                    }
                }
                if font.is_none() {
                    font = self.best_match(fam, 80, 100, 0);
                }
                if font.is_some() {
                    break;
                }
            }
            if pass == 0 {
                self.search(name);
            }
        }
        let mut font = font?;
        let parent = self.fonts[font].parent;
        self.req_engine = 0;
        let (mut req_bold, mut req_ital) = (false, false);
        if let Some(variant) = variant {
            let v = variant.as_bytes().to_vec();
            let mut var_string = String::new();
            let mut cp = 0usize;
            let append = |vs: &mut String, s: &str| {
                if !vs.is_empty() && !vs.ends_with('/') {
                    vs.push('/');
                }
                vs.push_str(s);
            };
            while cp < v.len() {
                if starts(&v, cp, b"AAT") {
                    self.req_engine = b'A';
                    cp += 3;
                    append(&mut var_string, "AAT");
                } else if starts(&v, cp, b"ICU") {
                    self.req_engine = b'O';
                    cp += 3;
                    append(&mut var_string, "OT");
                } else if starts(&v, cp, b"OT") {
                    self.req_engine = b'O';
                    cp += 2;
                    append(&mut var_string, "OT");
                } else if starts(&v, cp, b"GR") {
                    self.req_engine = b'G';
                    cp += 2;
                    append(&mut var_string, "GR");
                } else if v[cp] == b'S' {
                    cp += 1;
                    if v.get(cp) == Some(&b'=') {
                        cp += 1;
                    }
                    pt_size = 0.0;
                    while let Some(&c @ b'0'..=b'9') = v.get(cp) {
                        pt_size = pt_size * 10.0 + f64::from(c - b'0');
                        cp += 1;
                    }
                    if v.get(cp) == Some(&b'.') {
                        let mut dec = 1.0f64;
                        cp += 1;
                        while let Some(&c @ b'0'..=b'9') = v.get(cp) {
                            dec *= 10.0;
                            pt_size += f64::from(c - b'0') / dec;
                            cp += 1;
                        }
                    }
                } else {
                    loop {
                        match v.get(cp) {
                            Some(b'B') => {
                                req_bold = true;
                                cp += 1;
                            }
                            Some(b'I') => {
                                req_ital = true;
                                cp += 1;
                            }
                            _ => break,
                        }
                    }
                }
                while cp < v.len() && v[cp] != b'/' {
                    cp += 1;
                }
                if cp < v.len() && v[cp] == b'/' {
                    cp += 1;
                }
            }
            *variant = var_string;
            if let Some(parent) = parent {
                if req_ital {
                    font = self.apply_italic(font, parent);
                }
                if req_bold {
                    font = self.apply_bold(font, parent);
                }
            }
        }
        if pt_size < 0.0 {
            pt_size = dsize;
        }
        let f = &self.fonts[font];
        if f.op_size.sub_family_id != 0
            && pt_size > 0.0
            && let Some(parent) = parent
        {
            let mut best_mismatch =
                (f.op_size.min_size - pt_size).max(pt_size - f.op_size.max_size);
            if best_mismatch > 0.0 {
                let sub = f.op_size.sub_family_id;
                let mut best = font;
                for &g in self.families[parent].styles.values() {
                    let o = self.fonts[g].op_size;
                    if o.sub_family_id != sub {
                        continue;
                    }
                    let mismatch = (o.min_size - pt_size).max(pt_size - o.max_size);
                    if mismatch < best_mismatch {
                        best = g;
                        best_mismatch = mismatch;
                    }
                    if best_mismatch <= 0.0 {
                        break;
                    }
                }
                font = best;
            }
        }
        let f = &self.fonts[font];
        if f.op_size.design_size != 0.0 {
            design_size = (f.op_size.design_size * 65536.0 + 0.5) as u32 as i32;
        }
        let mut diagnostics = Vec::new();
        if self.tracing_fonts > 0 {
            diagnostics.push(Diagnostic::FontPath(
                self.index.entries[f.entry].path.to_string(),
            ));
        }
        Some(Found {
            font,
            design_size,
            diagnostics,
        })
    }

    fn apply_italic(&self, font: usize, parent: usize) -> usize {
        let f = &self.fonts[font];
        let p = &self.families[parent];
        let mut best = Some(font);
        if f.slant < p.max_slant {
            best = self.best_match(
                parent,
                i32::from(f.weight),
                i32::from(f.width),
                i32::from(p.max_slant),
            );
        }
        if best == Some(font) && f.slant > p.min_slant {
            best = self.best_match(
                parent,
                i32::from(f.weight),
                i32::from(f.width),
                i32::from(p.min_slant),
            );
        }
        if p.min_weight == p.max_weight
            && let Some(b) = best
            && self.fonts[b].is_bold != f.is_bold
        {
            let mut new_best = None;
            for &g in p.styles.values() {
                let gf = &self.fonts[g];
                if gf.is_bold == f.is_bold && new_best.is_none() && gf.is_italic != f.is_italic {
                    new_best = Some(g);
                    break;
                }
            }
            if new_best.is_some() {
                best = new_best;
            }
        }
        if best == Some(font) {
            best = None;
            for &g in p.styles.values() {
                let gf = &self.fonts[g];
                if gf.is_italic == !f.is_italic {
                    if p.min_weight != p.max_weight {
                        if best.is_none_or(|b| {
                            Self::weight_and_width_diff(gf, f)
                                < Self::weight_and_width_diff(&self.fonts[b], f)
                        }) {
                            best = Some(g);
                        }
                    } else if best.is_none() && gf.is_bold == f.is_bold {
                        best = Some(g);
                        break;
                    }
                }
            }
        }
        best.unwrap_or(font)
    }

    fn apply_bold(&self, font: usize, parent: usize) -> usize {
        let f = &self.fonts[font];
        let p = &self.families[parent];
        let mut best = font;
        if f.weight < p.max_weight {
            best = self
                .best_match(
                    parent,
                    i32::from(f.weight)
                        + (i32::from(p.max_weight) - i32::from(p.min_weight)) / 2
                        + 1,
                    i32::from(f.width),
                    i32::from(f.slant),
                )
                .unwrap_or(font);
            if p.min_slant == p.max_slant {
                let mut new_best: Option<usize> = None;
                for &g in p.styles.values() {
                    let gf = &self.fonts[g];
                    if gf.is_italic == f.is_italic
                        && new_best.is_none_or(|n| {
                            Self::weight_and_width_diff(gf, &self.fonts[best])
                                < Self::weight_and_width_diff(&self.fonts[n], &self.fonts[best])
                        })
                    {
                        new_best = Some(g);
                    }
                }
                if let Some(n) = new_best {
                    best = n;
                }
            }
        }
        if best == font && !f.is_bold {
            for &g in p.styles.values() {
                let gf = &self.fonts[g];
                if gf.is_italic == f.is_italic && gf.is_bold {
                    best = g;
                    break;
                }
            }
        }
        best
    }
}
