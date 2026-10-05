//! A face, parsed once and shared by content, answering as FreeType 2.14
//! does with `FT_LOAD_NO_SCALE` (XeTeX's `XeTeXFontInst.cpp`).
//!
//! FreeType's numbers, not the tables' raw ones, where they differ:
//! - the ascender and descender of `FT_Face` (hhea, or OS/2's typo values
//!   when `USE_TYPO_METRICS` is set or hhea's are zero, then win values);
//! - the character map FreeType selects (`find_unicode_charmap`: the last
//!   UCS-4 subtable, else the last Unicode one; a symbol-only font maps
//!   nothing), and `FT_Get_Char_Index`'s check against `num_glyphs`;
//! - the control box of the unscaled outline (`FT_Glyph_Get_CBox` with
//!   `FT_GLYPH_BBOX_UNSCALED`): every point, control points included; a
//!   TrueType outline moved by `pp1.x` (its `xMin` minus its left side
//!   bearing), a CFF one's 16.16 points floored (`x >> 10` of cf2's
//!   26.6-by-1024 scale);
//! - `num_glyphs`: maxp's, or the CFF CharStrings count for a CFF font.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::{Tag, tag};

/// A face's identity: its bytes (hashed) and its index in the file. Two
/// faces with the same key answer every query the same way, so shaping
/// and metrics can be memoized on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceKey {
    /// A 128-bit hash of the file's bytes.
    pub hash: [u64; 2],
    /// The file's length.
    pub len: u64,
    /// The face index (`FT_New_Face`'s `face_index`).
    pub index: u32,
}

/// The 128-bit content hash used in [`FaceKey`] (two multiply-rotate lanes
/// over little-endian words; for memo keys, not security).
#[must_use]
pub fn content_hash(data: &[u8]) -> [u64; 2] {
    const K1: u64 = 0x9E37_79B9_7F4A_7C15;
    const K2: u64 = 0xC2B2_AE3D_27D4_EB4F;
    let mut a: u64 = 0x2545_F491_4F6C_DD1D ^ data.len() as u64;
    let mut b: u64 = 0x1656_67B1_9E37_79F9;
    let mut chunks = data.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        a = (a ^ w).wrapping_mul(K1).rotate_left(29);
        b = (b.wrapping_add(w)).wrapping_mul(K2).rotate_left(31) ^ a;
    }
    for &byte in chunks.remainder() {
        a = (a ^ u64::from(byte)).wrapping_mul(K1).rotate_left(29);
        b = b.wrapping_add(u64::from(byte)).wrapping_mul(K2) ^ a;
    }
    a ^= a >> 33;
    a = a.wrapping_mul(K2);
    b ^= b >> 29;
    b = b.wrapping_mul(K1);
    [a ^ (b >> 17), b ^ (a >> 23)]
}

/// A glyph's control box in font units (`FT_BBox`, unscaled).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BBox {
    pub x_min: i32,
    pub y_min: i32,
    pub x_max: i32,
    pub y_max: i32,
}

/// The outline format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outlines {
    /// `glyf`/`loca`.
    TrueType,
    /// `CFF ` (CID-keyed or not).
    Cff { cid: bool },
    /// `CFF2`.
    Cff2,
}

/// OS/2's fields XeTeX reads (FreeType zeroes the cap and x heights of a
/// version 0 or 1 table).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Os2 {
    pub version: u16,
    pub weight_class: u16,
    pub width_class: u16,
    pub fs_selection: u16,
    pub typo_ascender: i16,
    pub typo_descender: i16,
    pub win_ascent: u16,
    pub win_descent: u16,
    pub x_height: i16,
    pub cap_height: i16,
}

/// A parsed face. Construct with [`Face::new`]; share as `Arc<Face>`.
pub struct Face {
    data: Arc<[u8]>,
    index: u32,
    key: FaceKey,
    /// Table directory: (tag, offset, length), offsets into `data`.
    tables: Vec<(Tag, u32, u32)>,
    font: harfrust::Font,
    outlines: Outlines,
    upem: u16,
    num_glyphs: u32,
    ascender: i16,
    descender: i16,
    os2: Option<Os2>,
    italic_angle: Option<i32>,
    mac_style: Option<u16>,
    /// The selected Unicode subtable (offset into `data`), if any.
    cmap: Option<u32>,
    /// The format 14 subtable (variation sequences), if any.
    cmap14: Option<u32>,
    /// `hmtx`: (offset, length, numberOfHMetrics).
    hmtx: Option<(u32, u32, u16)>,
    /// `vmtx`: (offset, length, numberOfVMetrics).
    vmtx: Option<(u32, u32, u16)>,
    has_glyph_names: bool,
    /// Control boxes computed so far, packed (see [`pack_bbox`]).
    bbox_cache: Box<[AtomicU64]>,
}

const BBOX_EMPTY: u64 = u64::MAX;

fn pack_bbox(b: BBox) -> Option<u64> {
    let f = |v: i32| i16::try_from(v).ok().map(|v| u64::from(v as u16));
    let p = f(b.x_min)? | f(b.y_min)? << 16 | f(b.x_max)? << 32 | f(b.y_max)? << 48;
    (p != BBOX_EMPTY).then_some(p)
}

fn unpack_bbox(p: u64) -> BBox {
    let g = |s: u32| i32::from((p >> s) as u16 as i16);
    BBox {
        x_min: g(0),
        y_min: g(16),
        x_max: g(32),
        y_max: g(48),
    }
}

pub(crate) fn rd_u16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
pub(crate) fn rd_i16(d: &[u8], o: usize) -> Option<i16> {
    rd_u16(d, o).map(|v| v as i16)
}
pub(crate) fn rd_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *d.get(o)?,
        *d.get(o + 1)?,
        *d.get(o + 2)?,
        *d.get(o + 3)?,
    ]))
}
pub(crate) fn rd_u24(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        0,
        *d.get(o)?,
        *d.get(o + 1)?,
        *d.get(o + 2)?,
    ]))
}

/// The offset of face `index`'s table directory in `data` (a TrueType
/// collection's entry, or 0 for a single font and index 0).
pub(crate) fn sfnt_offset(data: &[u8], index: u32) -> Option<usize> {
    if data.get(0..4)? == b"ttcf" {
        let n = rd_u32(data, 8)?;
        if index >= n {
            return None;
        }
        Some(rd_u32(data, 12 + 4 * index as usize)? as usize)
    } else if index == 0 {
        Some(0)
    } else {
        None
    }
}

/// The table directory of the face at `offset`.
pub(crate) fn table_directory(data: &[u8], offset: usize) -> Option<Vec<(Tag, u32, u32)>> {
    let version = rd_u32(data, offset)?;
    if version != 0x0001_0000 && version != tag(b"OTTO") && version != tag(b"true") {
        return None;
    }
    let n = rd_u16(data, offset + 4)? as usize;
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let r = offset + 12 + 16 * i;
        let t = rd_u32(data, r)?;
        let off = rd_u32(data, r + 8)?;
        let len = rd_u32(data, r + 12)?;
        if (off as usize).checked_add(len as usize)? > data.len() {
            // FreeType drops a table that runs past the file's end.
            continue;
        }
        v.push((t, off, len));
    }
    Some(v)
}

impl Face {
    /// `FT_New_Face(path, index)` on the file's bytes, as XeTeXFontInst's
    /// `initialize` takes it: `None` where that fails or the face is not
    /// scalable (no outlines).
    #[must_use]
    pub fn new(data: Arc<[u8]>, index: u32) -> Option<Arc<Face>> {
        let key = FaceKey {
            hash: content_hash(&data),
            len: data.len() as u64,
            index,
        };
        Self::with_key(data, index, key)
    }

    /// [`Face::new`] with a key the caller already has (for a Host that
    /// hashes its files itself).
    #[must_use]
    pub fn with_key(data: Arc<[u8]>, index: u32, key: FaceKey) -> Option<Arc<Face>> {
        // FreeType opens a WOFF file as the sfnt it holds.
        let data: Arc<[u8]> = if crate::woff::is_woff(&data) {
            Arc::from(crate::woff::decode(&data)?)
        } else {
            data
        };
        let off = sfnt_offset(&data, index & 0xFFFF)?;
        let tables = table_directory(&data, off)?;
        let find = |t: &[u8; 4]| tables.iter().find(|e| e.0 == tag(t)).map(|e| (e.1, e.2));
        // FreeType reads a table's fields from the file, past the table's
        // length if need be (`FT_STREAM_READ_FIELDS`): only the file's end
        // limits them.
        let (head_off, _) = find(b"head")?;
        let d: &[u8] = &data;
        if head_off as usize + 54 > d.len() {
            return None;
        }
        let head = head_off as usize;
        let upem = rd_u16(d, head + 18)?;
        let mac_style = rd_u16(d, head + 44);
        let (maxp_off, _) = find(b"maxp")?;
        let maxp_glyphs = u32::from(rd_u16(d, maxp_off as usize + 4)?);
        let outlines = if find(b"glyf").is_some() && find(b"loca").is_some() {
            Outlines::TrueType
        } else if let Some((cff, len)) = find(b"CFF ") {
            Outlines::Cff {
                cid: cff_is_cid(&d[cff as usize..(cff + len) as usize]).unwrap_or(false),
            }
        } else if find(b"CFF2").is_some() {
            Outlines::Cff2
        } else {
            // Bitmap-only: not `FT_IS_SCALABLE`, XeTeX refuses it.
            return None;
        };
        let num_glyphs = match outlines {
            Outlines::Cff { .. } => {
                let (cff, len) = find(b"CFF ")?;
                cff_charstrings_count(&d[cff as usize..(cff + len) as usize])?
            }
            _ => maxp_glyphs,
        };
        let os2 = find(b"OS/2").and_then(|(o, _)| read_os2(&d[o as usize..]));
        let hhea = find(b"hhea");
        let (hh_asc, hh_desc, n_hmetrics) = match hhea {
            Some((o, _)) if o as usize + 36 <= d.len() => {
                let o = o as usize;
                (rd_i16(d, o + 4)?, rd_i16(d, o + 6)?, rd_u16(d, o + 34)?)
            }
            _ => (0, 0, 0),
        };
        // sfobjs.c: the ascender and descender of `FT_Face`.
        let (ascender, descender) = match os2 {
            Some(os) if os.fs_selection & 128 != 0 => (os.typo_ascender, os.typo_descender),
            _ => {
                if hh_asc == 0 && hh_desc == 0 {
                    match os2 {
                        Some(os) if os.typo_ascender != 0 || os.typo_descender != 0 => {
                            (os.typo_ascender, os.typo_descender)
                        }
                        Some(os) => (os.win_ascent as i16, -(os.win_descent as i16)),
                        None => (0, 0),
                    }
                } else {
                    (hh_asc, hh_desc)
                }
            }
        };
        let post = find(b"post");
        let italic_angle = post
            .filter(|&(o, _)| o as usize + 32 <= d.len())
            .and_then(|(o, _)| rd_u32(d, o as usize + 4).map(|v| v as i32));
        let post_version = post.and_then(|(o, _)| rd_u32(d, o as usize));
        let tt_names = post_version.is_some_and(|v| v != 0x0003_0000);
        let has_glyph_names = match outlines {
            Outlines::Cff { cid } => tt_names || !cid,
            Outlines::Cff2 => tt_names,
            Outlines::TrueType => tt_names,
        };
        let hmtx = find(b"hmtx").map(|(o, l)| (o, l, n_hmetrics));
        let vmtx = match (find(b"vhea"), find(b"vmtx")) {
            (Some((vo, vl)), Some((o, l))) if vl >= 36 => {
                Some((o, l, rd_u16(d, vo as usize + 34).unwrap_or(0)))
            }
            _ => None,
        };
        let (cmap, cmap14) = match find(b"cmap") {
            Some((o, l)) => select_cmap(d, o as usize, l as usize),
            None => (None, None),
        };
        let font = harfrust::Font::new(
            read_fonts::model::Blob::from(
                Arc::new(ArcBytes(data.clone())) as Arc<dyn AsRef<[u8]> + Send + Sync>
            ),
            index & 0xFFFF,
        )?;
        let n_cache = num_glyphs.min(0x1_0000) as usize;
        let bbox_cache = (0..n_cache).map(|_| AtomicU64::new(BBOX_EMPTY)).collect();
        Some(Arc::new(Face {
            data,
            index,
            key,
            tables,
            font,
            outlines,
            upem,
            num_glyphs,
            ascender,
            descender,
            os2,
            italic_angle,
            mac_style,
            cmap,
            cmap14,
            hmtx,
            vmtx,
            has_glyph_names,
            bbox_cache,
        }))
    }

    /// The face's content key.
    #[must_use]
    pub fn key(&self) -> FaceKey {
        self.key
    }

    /// The file's bytes.
    #[must_use]
    pub fn data(&self) -> &Arc<[u8]> {
        &self.data
    }

    /// The face index the face was opened with.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The `read-fonts` model of the face (what HarfRust shapes with).
    #[must_use]
    pub fn font(&self) -> &harfrust::Font {
        &self.font
    }

    /// A `read-fonts` view of the face's tables.
    #[must_use]
    pub fn font_ref(&self) -> Option<read_fonts::FontRef<'_>> {
        read_fonts::FontRef::from_index(&self.data, self.index & 0xFFFF).ok()
    }

    /// The bytes of table `t` (`FT_Load_Sfnt_Table`).
    #[must_use]
    pub fn table(&self, t: Tag) -> Option<&[u8]> {
        let &(_, o, l) = self.tables.iter().find(|e| e.0 == t)?;
        self.data.get(o as usize..(o + l) as usize)
    }

    /// The outline format.
    #[must_use]
    pub fn outlines(&self) -> Outlines {
        self.outlines
    }

    /// `units_per_EM`.
    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        self.upem
    }

    /// `FT_Face`'s `num_glyphs`.
    #[must_use]
    pub fn num_glyphs(&self) -> u32 {
        self.num_glyphs
    }

    /// `FT_Face`'s `ascender`, in font units.
    #[must_use]
    pub fn ascender(&self) -> i16 {
        self.ascender
    }

    /// `FT_Face`'s `descender`, in font units (negative below the baseline).
    #[must_use]
    pub fn descender(&self) -> i16 {
        self.descender
    }

    /// The OS/2 table's fields, if the face has one
    /// (`FT_Get_Sfnt_Table(ft_sfnt_os2)`).
    #[must_use]
    pub fn os2(&self) -> Option<Os2> {
        self.os2
    }

    /// `post`'s italic angle (16.16), if the face has a `post` table.
    #[must_use]
    pub fn italic_angle(&self) -> Option<i32> {
        self.italic_angle
    }

    /// `head`'s `macStyle`.
    #[must_use]
    pub fn mac_style(&self) -> Option<u16> {
        self.mac_style
    }

    /// `FT_HAS_GLYPH_NAMES`.
    #[must_use]
    pub fn has_glyph_names(&self) -> bool {
        self.has_glyph_names
    }

    /// `FT_Get_Char_Index` on the Unicode charmap FreeType selected: 0 for
    /// none, and for a glyph id not below `num_glyphs`.
    #[must_use]
    pub fn char_index(&self, c: u32) -> u32 {
        let Some(sub) = self.cmap else { return 0 };
        let g = cmap_lookup(&self.data, sub as usize, c).unwrap_or(0);
        if g >= self.num_glyphs { 0 } else { g }
    }

    /// `FT_Face_GetCharVariantIndex`: the glyph of `c` followed by variation
    /// selector `vs` (format 14; a default sequence maps through the
    /// Unicode charmap), or 0.
    #[must_use]
    pub fn char_variant_index(&self, c: u32, vs: u32) -> u32 {
        let Some(sub) = self.cmap14 else { return 0 };
        match cmap14_lookup(&self.data, sub as usize, c, vs) {
            Some(VariantGlyph::Default) => self.char_index(c),
            Some(VariantGlyph::Glyph(g)) if g < self.num_glyphs => g,
            _ => 0,
        }
    }

    /// The characters mapped to a glyph (`FT_Get_First_Char` …
    /// `FT_Get_Next_Char`): the first and the last, or 0 for none.
    #[must_use]
    pub fn char_range(&self) -> (u32, u32) {
        let Some(sub) = self.cmap else { return (0, 0) };
        let mut first = None;
        let mut last = 0;
        cmap_for_each(&self.data, sub as usize, &mut |c, g| {
            if g != 0 && g < self.num_glyphs {
                if first.is_none_or(|f| c < f) {
                    first = Some(c);
                }
                if c > last {
                    last = c;
                }
            }
        });
        (first.unwrap_or(0), last)
    }

    /// `FT_Get_Advance(face, gid, FT_LOAD_NO_SCALE)`: the `hmtx` advance in
    /// font units, `None` for a glyph id past `num_glyphs`.
    #[must_use]
    pub fn h_advance(&self, gid: u32) -> Option<u32> {
        if gid >= self.num_glyphs {
            return None;
        }
        let Some((o, l, n)) = self.hmtx else {
            return Some(0);
        };
        if n == 0 {
            return Some(0);
        }
        let i = gid.min(u32::from(n) - 1) as usize;
        if 4 * i + 2 > l as usize {
            return Some(0);
        }
        Some(u32::from(
            rd_u16(&self.data, o as usize + 4 * i).unwrap_or(0),
        ))
    }

    /// `FT_Get_Advance(…, FT_LOAD_NO_SCALE | FT_LOAD_VERTICAL_LAYOUT)`: the
    /// `vmtx` advance, or FreeType's made-up one (OS/2's typo ascender
    /// minus descender, else hhea's).
    #[must_use]
    pub fn v_advance(&self, gid: u32) -> Option<u32> {
        if gid >= self.num_glyphs {
            return None;
        }
        if let Some((o, l, n)) = self.vmtx
            && n > 0
        {
            let i = gid.min(u32::from(n) - 1) as usize;
            if 4 * i + 2 <= l as usize {
                return Some(u32::from(
                    rd_u16(&self.data, o as usize + 4 * i).unwrap_or(0),
                ));
            }
            return Some(0);
        }
        Some(match self.os2 {
            Some(os) => (i32::from(os.typo_ascender) - i32::from(os.typo_descender)) as u32,
            None => (i32::from(self.ascender) - i32::from(self.descender)) as u32,
        })
    }

    /// The control box of glyph `gid`'s unscaled outline: `None` where
    /// `FT_Load_Glyph` fails (XeTeX then keeps zeros); all zeros for an
    /// empty glyph.
    #[must_use]
    pub fn cbox(&self, gid: u32) -> Option<BBox> {
        if gid >= self.num_glyphs {
            return None;
        }
        if let Some(slot) = self.bbox_cache.get(gid as usize) {
            let p = slot.load(Ordering::Relaxed);
            if p != BBOX_EMPTY {
                return Some(unpack_bbox(p));
            }
        }
        let b = self.compute_cbox(gid)?;
        if let (Some(slot), Some(p)) = (self.bbox_cache.get(gid as usize), pack_bbox(b)) {
            slot.store(p, Ordering::Relaxed);
        }
        Some(b)
    }

    fn compute_cbox(&self, gid: u32) -> Option<BBox> {
        use skrifa::MetadataProvider;
        use skrifa::instance::{LocationRef, Size};
        let font = self.font_ref()?;
        let outlines = font.outline_glyphs();
        let glyph = outlines.get(skrifa::GlyphId::new(gid))?;
        match self.outlines {
            Outlines::TrueType => glyph
                .with_scaled_glyf_outline(Size::unscaled(), LocationRef::default(), None, |o| {
                    let mut b: Option<BBox> = None;
                    for p in o.points.iter() {
                        // 26.6 values of whole font units.
                        let (x, y) = (p.x.to_bits() >> 6, p.y.to_bits() >> 6);
                        b = Some(match b {
                            None => BBox {
                                x_min: x,
                                y_min: y,
                                x_max: x,
                                y_max: y,
                            },
                            Some(b) => BBox {
                                x_min: b.x_min.min(x),
                                y_min: b.y_min.min(y),
                                x_max: b.x_max.max(x),
                                y_max: b.y_max.max(y),
                            },
                        });
                    }
                    Ok(b.unwrap_or_default())
                })
                .ok(),
            _ => {
                let mut pen = CboxPen::default();
                let settings = skrifa::outline::DrawSettings::unhinted(
                    Size::unscaled(),
                    LocationRef::default(),
                );
                glyph.draw(settings, &mut pen).ok()?;
                Some(pen.bbox.unwrap_or_default())
            }
        }
    }

    /// `FT_Get_Glyph_Name`: the glyph's name (the CFF charset's for a CFF
    /// font, `post`'s otherwise), or `None` without names.
    #[must_use]
    pub fn glyph_name(&self, gid: u32) -> Option<Vec<u8>> {
        if !self.has_glyph_names || gid >= self.num_glyphs {
            return None;
        }
        let font = self.font_ref()?;
        let mut name: Vec<u8> = match self.outlines {
            Outlines::Cff { .. } => {
                use read_fonts::TableProvider;
                let cff = font.cff().ok()?;
                let charset = cff.charset(0)?;
                let sid = charset.string_id(read_fonts::types::GlyphId::new(gid))?;
                cff.string(sid)?.to_vec()
            }
            _ => {
                use read_fonts::TableProvider;
                let d = self.table(tag(b"post"))?;
                let version = rd_u32(d, 0)?;
                if version == 0x0002_0000 {
                    let n = u32::from(rd_u16(d, 32)?);
                    if gid >= n {
                        return None;
                    }
                    let idx = rd_u16(d, 34 + 2 * gid as usize)?;
                    if idx >= 258 {
                        // the custom names: Pascal strings after the indices
                        let mut p = 34 + 2 * n as usize;
                        for _ in 0..idx - 258 {
                            p += 1 + *d.get(p)? as usize;
                        }
                        let len = *d.get(p)? as usize;
                        d.get(p + 1..p + 1 + len)?.to_vec()
                    } else {
                        let post = font.post().ok()?;
                        post.glyph_name(read_fonts::types::GlyphId16::new(gid as u16))?
                            .as_bytes()
                            .to_vec()
                    }
                } else {
                    let post = font.post().ok()?;
                    post.glyph_name(read_fonts::types::GlyphId16::new(gid as u16))?
                        .as_bytes()
                        .to_vec()
                }
            }
        };
        // `FT_Get_Glyph_Name` copies into XeTeX's 256-byte buffer.
        name.truncate(255);
        Some(name)
    }

    /// `FT_Get_Name_Index`: the first glyph named `name`, or 0.
    #[must_use]
    pub fn glyph_index_by_name(&self, name: &[u8]) -> u32 {
        if !self.has_glyph_names {
            return 0;
        }
        (0..self.num_glyphs)
            .find(|&g| self.glyph_name(g).as_deref() == Some(name))
            .unwrap_or(0)
    }
}

/// A `Blob` source over the shared bytes.
struct ArcBytes(Arc<[u8]>);

impl AsRef<[u8]> for ArcBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Collects the control box of a CFF outline as FreeType's cf2 builder
/// sees it: a point is added only once a contour draws (a lone `moveto`
/// adds nothing), and coordinates are floored to font units.
#[derive(Default)]
struct CboxPen {
    pending: Option<(f32, f32)>,
    bbox: Option<BBox>,
}

impl CboxPen {
    fn add(&mut self, x: f32, y: f32) {
        let (x, y) = (libm::floorf(x) as i32, libm::floorf(y) as i32);
        self.bbox = Some(match self.bbox {
            None => BBox {
                x_min: x,
                y_min: y,
                x_max: x,
                y_max: y,
            },
            Some(b) => BBox {
                x_min: b.x_min.min(x),
                y_min: b.y_min.min(y),
                x_max: b.x_max.max(x),
                y_max: b.y_max.max(y),
            },
        });
    }
    fn start(&mut self) {
        if let Some((x, y)) = self.pending.take() {
            self.add(x, y);
        }
    }
}

impl skrifa::outline::OutlinePen for CboxPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pending = Some((x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.start();
        self.add(x, y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.start();
        self.add(cx0, cy0);
        self.add(x, y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.start();
        self.add(cx0, cy0);
        self.add(cx1, cy1);
        self.add(x, y);
    }
    fn close(&mut self) {
        self.pending = None;
    }
}

pub(crate) fn read_os2(d: &[u8]) -> Option<Os2> {
    // `tt_face_load_os2`: the version 0 fields (78 bytes), then 8 more for
    // version 1, 10 more for 2 to 4, 4 more for 5, read from the file (`d`
    // runs to its end); a file that ends first means no OS/2 table.
    let version = rd_u16(d, 0)?;
    let need = match version {
        0 => 78,
        1 => 86,
        2..=4 => 96,
        _ => 100,
    };
    if d.len() < need {
        return None;
    }
    let (x_height, cap_height) = if version >= 2 {
        (rd_i16(d, 86)?, rd_i16(d, 88)?)
    } else {
        (0, 0)
    };
    Some(Os2 {
        version,
        weight_class: rd_u16(d, 4)?,
        width_class: rd_u16(d, 6)?,
        fs_selection: rd_u16(d, 62)?,
        typo_ascender: rd_i16(d, 68)?,
        typo_descender: rd_i16(d, 70)?,
        win_ascent: rd_u16(d, 74)?,
        win_descent: rd_u16(d, 76)?,
        x_height,
        cap_height,
    })
}

/// Reads a CFF INDEX at `o`: (count, offSize, offsets base, data base).
fn cff_index(d: &[u8], o: usize) -> Option<(u32, usize, usize, usize)> {
    let count = u32::from(rd_u16(d, o)?);
    if count == 0 {
        return Some((0, 0, o + 2, o + 2));
    }
    let off_size = *d.get(o + 2)? as usize;
    let offs = o + 3;
    let data = offs + (count as usize + 1) * off_size - 1;
    Some((count, off_size, offs, data))
}

fn cff_index_end(d: &[u8], o: usize) -> Option<usize> {
    let (count, off_size, offs, data) = cff_index(d, o)?;
    if count == 0 {
        return Some(o + 2);
    }
    let last = cff_offset(d, offs + count as usize * off_size, off_size)?;
    Some(data + last)
}

fn cff_offset(d: &[u8], o: usize, size: usize) -> Option<usize> {
    let mut v = 0usize;
    for i in 0..size {
        v = v << 8 | *d.get(o + i)? as usize;
    }
    Some(v)
}

/// The first Top DICT's operators (operator → operands as integers).
fn cff_top_dict(d: &[u8]) -> Option<Vec<(u16, Vec<i32>)>> {
    let hdr = *d.get(2)? as usize;
    let names_end = cff_index_end(d, hdr)?;
    let (count, off_size, offs, data) = cff_index(d, names_end)?;
    if count == 0 {
        return None;
    }
    let start = data + cff_offset(d, offs, off_size)?;
    let end = data + cff_offset(d, offs + off_size, off_size)?;
    let dict = d.get(start..end)?;
    let mut out = Vec::new();
    let mut ops: Vec<i32> = Vec::new();
    let mut i = 0;
    while i < dict.len() {
        let b0 = dict[i];
        match b0 {
            0..=21 => {
                let op = if b0 == 12 {
                    i += 1;
                    1200 + u16::from(*dict.get(i)?)
                } else {
                    u16::from(b0)
                };
                out.push((op, core::mem::take(&mut ops)));
                i += 1;
            }
            28 => {
                ops.push(i32::from(rd_i16(dict, i + 1)?));
                i += 3;
            }
            29 => {
                ops.push(rd_u32(dict, i + 1)? as i32);
                i += 5;
            }
            30 => {
                // A real: skip its nibbles, keep a 0.
                i += 1;
                while i < dict.len() {
                    let b = dict[i];
                    i += 1;
                    if b & 0x0F == 0x0F || b >> 4 == 0x0F {
                        break;
                    }
                }
                ops.push(0);
            }
            32..=246 => {
                ops.push(i32::from(b0) - 139);
                i += 1;
            }
            247..=250 => {
                ops.push((i32::from(b0) - 247) * 256 + i32::from(*dict.get(i + 1)?) + 108);
                i += 2;
            }
            251..=254 => {
                ops.push(-(i32::from(b0) - 251) * 256 - i32::from(*dict.get(i + 1)?) - 108);
                i += 2;
            }
            _ => i += 1,
        }
    }
    Some(out)
}

fn cff_is_cid(d: &[u8]) -> Option<bool> {
    Some(cff_top_dict(d)?.iter().any(|(op, _)| *op == 1230))
}

fn cff_charstrings_count(d: &[u8]) -> Option<u32> {
    let dict = cff_top_dict(d)?;
    let (_, ops) = dict.iter().find(|(op, _)| *op == 17)?;
    let off = *ops.last()? as usize;
    Some(cff_index(d, off)?.0)
}

/// FreeType's charmap selection over the `cmap` table at `o`: the Unicode
/// subtable `find_unicode_charmap` picks, and the format 14 one.
fn select_cmap(d: &[u8], o: usize, len: usize) -> (Option<u32>, Option<u32>) {
    let Some(n) = rd_u16(d, o + 2) else {
        return (None, None);
    };
    let mut maps: Vec<(u16, u16, u32, u16)> = Vec::new();
    for i in 0..n as usize {
        let r = o + 4 + 8 * i;
        let (Some(p), Some(e), Some(off)) = (rd_u16(d, r), rd_u16(d, r + 2), rd_u32(d, r + 4))
        else {
            break;
        };
        if off as usize >= len {
            continue;
        }
        let sub = o + off as usize;
        let Some(fmt) = rd_u16(d, sub) else { continue };
        if !matches!(fmt, 0 | 2 | 4 | 6 | 8 | 10 | 12 | 13 | 14) {
            continue;
        }
        maps.push((p, e, sub as u32, fmt));
    }
    let unicode = |p: u16, e: u16| p == 0 || p == 2 || (p == 3 && (e == 1 || e == 10));
    let cmap14 = maps
        .iter()
        .find(|m| m.0 == 0 && m.1 == 5 && m.3 == 14)
        .map(|m| m.2);
    for m in maps.iter().rev() {
        if unicode(m.0, m.1)
            && ((m.0 == 3 && m.1 == 10)
                || (m.0 == 0 && m.1 == 4)
                || (m.0 == 0 && m.1 == 6 && m.3 == 13))
        {
            return (Some(m.2), cmap14);
        }
    }
    for m in maps.iter().rev() {
        if unicode(m.0, m.1) {
            return (Some(m.2), cmap14);
        }
    }
    (None, cmap14)
}

/// A subtable's glyph for character `c` (FreeType's `char_index`).
fn cmap_lookup(d: &[u8], o: usize, c: u32) -> Option<u32> {
    match rd_u16(d, o)? {
        0 => {
            if c < 256 {
                Some(u32::from(*d.get(o + 6 + c as usize)?))
            } else {
                Some(0)
            }
        }
        2 => cmap2_lookup(d, o, c),
        4 => cmap4_lookup(d, o, c),
        6 => {
            let first = u32::from(rd_u16(d, o + 6)?);
            let count = u32::from(rd_u16(d, o + 8)?);
            if c >= first && c - first < count {
                Some(u32::from(rd_u16(d, o + 10 + 2 * (c - first) as usize)?))
            } else {
                Some(0)
            }
        }
        10 => {
            let first = rd_u32(d, o + 12)?;
            let count = rd_u32(d, o + 16)?;
            if c >= first && c - first < count {
                Some(u32::from(rd_u16(d, o + 20 + 2 * (c - first) as usize)?))
            } else {
                Some(0)
            }
        }
        12 | 13 => {
            let fmt = rd_u16(d, o)?;
            let n = rd_u32(d, o + 12)? as usize;
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = usize::midpoint(lo, hi);
                let g = o + 16 + 12 * mid;
                let start = rd_u32(d, g)?;
                let end = rd_u32(d, g + 4)?;
                if c < start {
                    hi = mid;
                } else if c > end {
                    lo = mid + 1;
                } else {
                    let gid = rd_u32(d, g + 8)?;
                    return Some(if fmt == 12 {
                        gid.wrapping_add(c - start)
                    } else {
                        gid
                    });
                }
            }
            Some(0)
        }
        _ => Some(0),
    }
}

fn cmap2_lookup(d: &[u8], o: usize, c: u32) -> Option<u32> {
    if c > 0xFFFF {
        return Some(0);
    }
    let hi = (c >> 8) as usize;
    let lo = c & 0xFF;
    let key = rd_u16(d, o + 6 + 2 * hi)? as usize / 8;
    let subs = o + 6 + 512;
    let sh = subs + 8 * key;
    if key == 0 && c > 0xFF {
        return Some(0);
    }
    let code = if key == 0 { c } else { lo };
    let first = u32::from(rd_u16(d, sh)?);
    let count = u32::from(rd_u16(d, sh + 2)?);
    let delta = rd_i16(d, sh + 4)?;
    let range = rd_u16(d, sh + 6)? as usize;
    if code < first || code - first >= count {
        return Some(0);
    }
    let p = sh + 6 + range + 2 * (code - first) as usize;
    let g = rd_u16(d, p)?;
    if g == 0 {
        return Some(0);
    }
    Some(u32::from(g.wrapping_add(delta as u16)))
}

fn cmap4_lookup(d: &[u8], o: usize, c: u32) -> Option<u32> {
    if c > 0xFFFF {
        return Some(0);
    }
    let segs = rd_u16(d, o + 6)? as usize / 2;
    let ends = o + 14;
    let starts = ends + 2 * segs + 2;
    let deltas = starts + 2 * segs;
    let ranges = deltas + 2 * segs;
    let (mut lo, mut hi) = (0usize, segs);
    while lo < hi {
        let mid = usize::midpoint(lo, hi);
        let end = u32::from(rd_u16(d, ends + 2 * mid)?);
        let start = u32::from(rd_u16(d, starts + 2 * mid)?);
        if c < start {
            hi = mid;
        } else if c > end {
            lo = mid + 1;
        } else {
            let delta = rd_u16(d, deltas + 2 * mid)?;
            let range = rd_u16(d, ranges + 2 * mid)?;
            if range == 0xFFFF {
                return Some(0);
            }
            if range == 0 {
                return Some(u32::from((c as u16).wrapping_add(delta)));
            }
            let p = ranges + 2 * mid + range as usize + 2 * (c - start) as usize;
            let g = rd_u16(d, p).unwrap_or(0);
            if g == 0 {
                return Some(0);
            }
            return Some(u32::from(g.wrapping_add(delta)));
        }
    }
    Some(0)
}

/// Calls `f(char, glyph)` for every mapping of a Unicode subtable.
fn cmap_for_each(d: &[u8], o: usize, f: &mut dyn FnMut(u32, u32)) {
    let Some(fmt) = rd_u16(d, o) else { return };
    match fmt {
        4 => {
            let Some(segs) = rd_u16(d, o + 6).map(|v| v as usize / 2) else {
                return;
            };
            let ends = o + 14;
            let starts = ends + 2 * segs + 2;
            for s in 0..segs {
                let (Some(end), Some(start)) = (rd_u16(d, ends + 2 * s), rd_u16(d, starts + 2 * s))
                else {
                    return;
                };
                if start > end {
                    continue;
                }
                for c in u32::from(start)..=u32::from(end) {
                    f(c, cmap4_lookup(d, o, c).unwrap_or(0));
                }
            }
        }
        12 | 13 => {
            let Some(n) = rd_u32(d, o + 12) else { return };
            for i in 0..n as usize {
                let g = o + 16 + 12 * i;
                let (Some(start), Some(end), Some(gid)) =
                    (rd_u32(d, g), rd_u32(d, g + 4), rd_u32(d, g + 8))
                else {
                    return;
                };
                if end < start || end > 0x10FFFF {
                    continue;
                }
                for c in start..=end {
                    f(
                        c,
                        if fmt == 12 {
                            gid.wrapping_add(c - start)
                        } else {
                            gid
                        },
                    );
                }
            }
        }
        0 => {
            for c in 0..256 {
                f(c, cmap_lookup(d, o, c).unwrap_or(0));
            }
        }
        6 | 10 => {
            let (first, count) = if fmt == 6 {
                (
                    rd_u16(d, o + 6).map(u32::from),
                    rd_u16(d, o + 8).map(u32::from),
                )
            } else {
                (rd_u32(d, o + 12), rd_u32(d, o + 16))
            };
            if let (Some(first), Some(count)) = (first, count) {
                for c in first..first.saturating_add(count) {
                    f(c, cmap_lookup(d, o, c).unwrap_or(0));
                }
            }
        }
        2 => {
            for c in 0..0x10000 {
                let g = cmap2_lookup(d, o, c).unwrap_or(0);
                if g != 0 {
                    f(c, g);
                }
            }
        }
        _ => {}
    }
}

enum VariantGlyph {
    Default,
    Glyph(u32),
}

fn cmap14_lookup(d: &[u8], o: usize, c: u32, vs: u32) -> Option<VariantGlyph> {
    let n = rd_u32(d, o + 6)? as usize;
    for i in 0..n {
        let r = o + 10 + 11 * i;
        if rd_u24(d, r)? != vs {
            continue;
        }
        let def = rd_u32(d, r + 3)? as usize;
        let non = rd_u32(d, r + 7)? as usize;
        if def != 0 {
            let t = o + def;
            let m = rd_u32(d, t)? as usize;
            for k in 0..m {
                let start = rd_u24(d, t + 4 + 4 * k)?;
                let add = u32::from(*d.get(t + 7 + 4 * k)?);
                if c >= start && c <= start + add {
                    return Some(VariantGlyph::Default);
                }
            }
        }
        if non != 0 {
            let t = o + non;
            let m = rd_u32(d, t)? as usize;
            for k in 0..m {
                if rd_u24(d, t + 4 + 5 * k)? == c {
                    return Some(VariantGlyph::Glyph(u32::from(rd_u16(d, t + 7 + 5 * k)?)));
                }
            }
        }
        return None;
    }
    None
}
