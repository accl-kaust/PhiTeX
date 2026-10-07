//! Font metrics: TFM files parsed into tables (tex.web part 30,
//! redesigned).
//!
//! A [`Font`] is immutable once loaded apart from its parameters (which
//! `\fontdimen` may change), so the engine shares it (`Arc`) and a cache
//! can keep it across jobs. Scaling follows §571–§572 exactly, and a file is
//! rejected exactly when tex.web would reject it (§560–§575).

use alloc::vec::Vec;

use crate::Scaled;

/// §544: what the remainder of a character's `char_info` means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tag {
    #[default]
    None,
    /// Its ligature/kern program starts at this index.
    Lig(u16),
    /// The next larger character.
    List(u8),
    /// An extensible recipe (index into [`Font::extensibles`]).
    Ext(u8),
}

crate::persist_enum!(Tag { None, Lig(a0), List(a0), Ext(a0) });

/// §543: one character's metric indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CharInfo {
    width: u8,
    height: u8,
    depth: u8,
    italic: u8,
    tag: u8,
    remainder: u8,
}

/// §545: one lig/kern instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LigKern {
    pub skip: u8,
    pub next: u8,
    pub op: u8,
    pub remainder: u8,
}

crate::persist_struct!(LigKern {
    skip,
    next,
    op,
    remainder
});

impl LigKern {
    /// §545: `stop_flag`, `kern_flag`.
    pub const STOP: u8 = 128;
    pub const KERN: u8 = 128;
}

/// §546: an extensible recipe (0 for an absent piece, except `rep`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extensible {
    pub top: u8,
    pub mid: u8,
    pub bot: u8,
    pub rep: u8,
}

crate::persist_struct!(Extensible { top, mid, bot, rep });

/// A TFM file tex.web would not load (§560's `abort`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BadTfm;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Font {
    pub check: [u8; 4],
    pub design_size: Scaled,
    pub size: Scaled,
    /// Smallest and largest character codes (`bc > ec` if there are none).
    pub bc: i32,
    pub ec: i32,
    /// The metrics of characters `bc..=ec`, resolved at load time so a
    /// lookup is one index.
    glyphs: Vec<Option<Glyph>>,
    pub lig_kerns: Vec<LigKern>,
    pub kerns: Vec<Scaled>,
    pub extensibles: Vec<Extensible>,
    /// `\fontdimen` 1…n at index 0…n-1 (at least seven).
    pub params: Vec<Scaled>,
    /// §545: the right boundary character (256 if none).
    pub bchar: i32,
    /// §549: `bchar` unless that character exists (then 256).
    pub false_bchar: i32,
    /// §545: where the left boundary program starts, if any.
    pub bchar_label: Option<u16>,
}

/// Saved with its tables in chunks named by their contents
/// (`persist::save_chunked`): a font changed in place (its `\fontdimen`s,
/// a character's tag) is a copy, and a copy then costs what changed.
impl crate::persist::Persist for Font {
    fn save(&self, s: &mut crate::persist::Saver) {
        use crate::persist::save_chunked;
        let Self {
            check,
            design_size,
            size,
            bc,
            ec,
            glyphs,
            lig_kerns,
            kerns,
            extensibles,
            params,
            bchar,
            false_bchar,
            bchar_label,
        } = self;
        check.save(s);
        for x in [design_size, size, bc, ec] {
            x.save(s);
        }
        save_chunked(glyphs, s);
        save_chunked(lig_kerns, s);
        save_chunked(kerns, s);
        save_chunked(extensibles, s);
        params.save(s);
        for x in [bchar, false_bchar] {
            x.save(s);
        }
        bchar_label.save(s);
    }
    fn load(l: &mut crate::persist::Loader) -> Option<Self> {
        use crate::persist::{Persist, load_chunked};
        let check = Persist::load(l)?;
        let (design_size, size, bc, ec) = Persist::load(l)?;
        let glyphs = load_chunked(l)?;
        let lig_kerns = load_chunked(l)?;
        let kerns = load_chunked(l)?;
        let extensibles = load_chunked(l)?;
        let (params, bchar, false_bchar, bchar_label) = Persist::load(l)?;
        Some(Self {
            check,
            design_size,
            size,
            bc,
            ec,
            glyphs,
            lig_kerns,
            kerns,
            extensibles,
            params,
            bchar,
            false_bchar,
            bchar_label,
        })
    }
}

/// Metrics of one existing character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Glyph {
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
    pub italic: Scaled,
    pub tag: Tag,
}

crate::persist_struct!(Glyph {
    width,
    height,
    depth,
    italic,
    tag
});

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<i32, BadTfm> {
        let b = *self.data.get(self.pos).ok_or(BadTfm)?;
        self.pos += 1;
        Ok(i32::from(b))
    }
    fn four(&mut self) -> Result<[u8; 4], BadTfm> {
        let w = self.data.get(self.pos..self.pos + 4).ok_or(BadTfm)?;
        self.pos += 4;
        Ok([w[0], w[1], w[2], w[3]])
    }
    /// §564: `read_sixteen`, rejecting values above 2^15.
    fn sixteen(&mut self) -> Result<i32, BadTfm> {
        let x = self.byte()?;
        if x > 127 {
            return Err(BadTfm);
        }
        Ok(x * 0o400 + self.byte()?)
    }
}

/// §571–§572: the scaling of `fix_word` `b` by size `z`.
struct Scaler {
    z: i32,
    alpha: i32,
    beta: i32,
}

impl Scaler {
    fn new(mut z: Scaled) -> Self {
        let mut alpha = 16;
        while z >= 0o40000000 {
            z /= 2;
            alpha += alpha;
        }
        let beta = 256 / alpha;
        Self {
            z,
            alpha: alpha * z,
            beta,
        }
    }
    /// §571: `store_scaled`.
    fn scale(&self, [a, b, c, d]: [u8; 4]) -> Result<Scaled, BadTfm> {
        let (b, c, d) = (i32::from(b), i32::from(c), i32::from(d));
        let z = self.z;
        let sw = (((((d * z) / 0o400) + (c * z)) / 0o400) + (b * z)) / self.beta;
        match a {
            0 => Ok(sw),
            255 => Ok(sw - self.alpha),
            _ => Err(BadTfm),
        }
    }
    /// `n` scaled words.
    fn table(&self, r: &mut Reader<'_>, n: i32) -> Result<Vec<Scaled>, BadTfm> {
        (0..n).map(|_| self.scale(r.four()?)).collect()
    }
}

impl Font {
    /// §552: the null font: no characters, seven zero parameters.
    #[must_use]
    pub fn null() -> Font {
        Font {
            check: [0; 4],
            design_size: 0,
            size: 0,
            bc: 1,
            ec: 0,
            glyphs: Vec::new(),
            lig_kerns: Vec::new(),
            kerns: Vec::new(),
            extensibles: Vec::new(),
            params: alloc::vec![0; 7],
            bchar: 256,
            false_bchar: 256,
            bchar_label: None,
        }
    }

    /// §565–§566: check a TFM file's size fields; the number of words
    /// tex.web's `font_info` would need for it (`lf`, at least seven
    /// parameters counted).
    pub fn memory_words(data: &[u8]) -> Result<i32, BadTfm> {
        let mut r = Reader { data, pos: 0 };
        let lf = r.sixteen()?;
        let lh = r.sixteen()?;
        let mut bc = r.sixteen()?;
        let mut ec = r.sixteen()?;
        if bc > ec + 1 || ec > 255 {
            return Err(BadTfm);
        }
        if bc > 255 {
            bc = 1;
            ec = 0;
        }
        let mut n = [0; 8];
        for x in &mut n {
            *x = r.sixteen()?;
        }
        let [nw, nh, nd, ni, nl, nk, ne, np] = n;
        if lf != 6 + lh + (ec - bc + 1) + nw + nh + nd + ni + nl + nk + ne + np {
            return Err(BadTfm);
        }
        if nw == 0 || nh == 0 || nd == 0 || ni == 0 {
            return Err(BadTfm);
        }
        Ok(lf - 6 - lh + (7 - np).max(0))
    }

    /// §568: the design size of a TFM file whose size fields are valid.
    pub fn design_size_of(data: &[u8]) -> Result<Scaled, BadTfm> {
        let mut r = Reader { data, pos: 2 };
        if r.sixteen()? < 2 {
            return Err(BadTfm); // `lh<2`
        }
        r.pos = 28;
        let mut z = r.sixteen()?;
        z = z * 0o400 + r.byte()?;
        z = z * 0o20 + r.byte()? / 0o20;
        if z < 0o200000 {
            return Err(BadTfm);
        }
        Ok(z)
    }

    /// Parse a TFM file at size `at` (`None`: the design size). Scaling by
    /// `\font\x=... scaled n` is the caller's (it can fail with an error
    /// TeX reports; see [`crate::scaled::xn_over_d`]).
    pub fn from_tfm(data: &[u8], at: Option<Scaled>) -> Result<Font, BadTfm> {
        let mut r = Reader { data, pos: 0 };
        // §565: read the TFM size fields.
        let lf = r.sixteen()?;
        let lh = r.sixteen()?;
        let mut bc = r.sixteen()?;
        let mut ec = r.sixteen()?;
        if bc > ec + 1 || ec > 255 {
            return Err(BadTfm);
        }
        if bc > 255 {
            // `bc=256` and `ec=255`
            bc = 1;
            ec = 0;
        }
        let nw = r.sixteen()?;
        let nh = r.sixteen()?;
        let nd = r.sixteen()?;
        let ni = r.sixteen()?;
        let nl = r.sixteen()?;
        let nk = r.sixteen()?;
        let ne = r.sixteen()?;
        let np = r.sixteen()?;
        if lf != 6 + lh + (ec - bc + 1) + nw + nh + nd + ni + nl + nk + ne + np {
            return Err(BadTfm);
        }
        if nw == 0 || nh == 0 || nd == 0 || ni == 0 {
            return Err(BadTfm);
        }
        // §568: read the TFM header.
        if lh < 2 {
            return Err(BadTfm);
        }
        let check = r.four()?;
        let mut z = r.sixteen()?; // this rejects a negative design size
        z = z * 0o400 + r.byte()?;
        z = z * 0o20 + r.byte()? / 0o20;
        if z < 0o200000 {
            return Err(BadTfm);
        }
        for _ in 2..lh {
            r.four()?; // ignore the rest of the header
        }
        let design_size = z;
        let size = at.unwrap_or(design_size);
        // §569: read character data.
        let n = usize::try_from(ec - bc + 1).unwrap_or(0);
        let mut chars = Vec::with_capacity(n);
        for k in 0..n {
            let [a, b, c, d] = r.four()?;
            let info = CharInfo {
                width: a,
                height: b / 16,
                depth: b % 16,
                italic: c / 4,
                tag: c % 4,
                remainder: d,
            };
            let d = i32::from(d);
            if i32::from(a) >= nw
                || i32::from(info.height) >= nh
                || i32::from(info.depth) >= nd
                || i32::from(info.italic) >= ni
            {
                return Err(BadTfm);
            }
            match info.tag {
                1 if d >= nl => return Err(BadTfm),
                3 if d >= ne => return Err(BadTfm),
                2 => {
                    // §570: check for charlist cycle.
                    if d < bc || d > ec {
                        return Err(BadTfm);
                    }
                    let current = i32::try_from(k).unwrap_or(0) + bc;
                    let mut d = d;
                    while d < current {
                        let q: &CharInfo = &chars[usize::try_from(d - bc).unwrap_or(0)];
                        if q.tag != 2 {
                            break;
                        }
                        d = i32::from(q.remainder); // next character on the list
                    }
                    if d == current {
                        return Err(BadTfm); // yes, there's a cycle
                    }
                }
                _ => {}
            }
            chars.push(info);
        }
        // §571: read box dimensions.
        let scaler = Scaler::new(size);
        let widths = scaler.table(&mut r, nw)?;
        let heights = scaler.table(&mut r, nh)?;
        let depths = scaler.table(&mut r, nd)?;
        let italics = scaler.table(&mut r, ni)?;
        if widths[0] != 0 || heights[0] != 0 || depths[0] != 0 || italics[0] != 0 {
            return Err(BadTfm); // width[0], height[0], depth[0], italic[0] must be zero
        }
        let exists = |c: i32| -> bool {
            c >= bc && c <= ec && chars[usize::try_from(c - bc).unwrap_or(0)].width > 0
        };
        // §573: read ligature/kern program.
        let mut bch_label = 0o77777;
        let mut bchar = 256;
        let mut lig_kerns = Vec::with_capacity(usize::try_from(nl).unwrap_or(0));
        for k in 0..nl {
            let [a, b, c, d] = r.four()?;
            let (ai, bi, ci, di) = (i32::from(a), i32::from(b), i32::from(c), i32::from(d));
            if ai > 128 {
                if 256 * ci + di >= nl {
                    return Err(BadTfm);
                }
                if ai == 255 && k == 0 {
                    bchar = bi;
                }
            } else {
                if bi != bchar && !exists(bi) {
                    return Err(BadTfm);
                }
                if ci < 128 {
                    if !exists(di) {
                        return Err(BadTfm); // check ligature
                    }
                } else if 256 * (ci - 128) + di >= nk {
                    return Err(BadTfm); // check kern
                }
                if ai < 128 && k + ai + 1 >= nl {
                    return Err(BadTfm);
                }
            }
            lig_kerns.push(LigKern {
                skip: a,
                next: b,
                op: c,
                remainder: d,
            });
        }
        if let Some(last) = lig_kerns.last()
            && last.skip == 255
        {
            bch_label = 256 * i32::from(last.op) + i32::from(last.remainder);
        }
        let kerns = scaler.table(&mut r, nk)?;
        // §574: read extensible character recipes.
        let mut extensibles = Vec::with_capacity(usize::try_from(ne).unwrap_or(0));
        for _ in 0..ne {
            let [a, b, c, d] = r.four()?;
            for piece in [a, b, c] {
                if piece != 0 && !exists(i32::from(piece)) {
                    return Err(BadTfm);
                }
            }
            if !exists(i32::from(d)) {
                return Err(BadTfm);
            }
            extensibles.push(Extensible {
                top: a,
                mid: b,
                bot: c,
                rep: d,
            });
        }
        // §575: read font parameters.
        let mut params = Vec::with_capacity(usize::try_from(np.max(7)).unwrap_or(7));
        for k in 1..=np {
            let w = r.four()?;
            if k == 1 {
                // the `slant` parameter is a pure number
                let mut sw = i32::from(w[0]);
                if sw > 127 {
                    sw -= 256;
                }
                sw = sw * 0o400 + i32::from(w[1]);
                sw = sw * 0o400 + i32::from(w[2]);
                params.push(sw * 0o20 + i32::from(w[3]) / 0o20);
            } else {
                params.push(scaler.scale(w)?);
            }
        }
        params.resize(params.len().max(7), 0);
        // §576: make final adjustments.
        let false_bchar = if exists(bchar) { 256 } else { bchar };
        let glyphs = chars
            .iter()
            .map(|i| {
                (i.width != 0).then(|| Glyph {
                    width: widths[usize::from(i.width)],
                    height: heights[usize::from(i.height)],
                    depth: depths[usize::from(i.depth)],
                    italic: italics[usize::from(i.italic)],
                    tag: match i.tag {
                        1 => Tag::Lig(u16::from(i.remainder)),
                        2 => Tag::List(i.remainder),
                        3 => Tag::Ext(i.remainder),
                        _ => Tag::None,
                    },
                })
            })
            .collect();
        Ok(Font {
            check,
            design_size,
            size,
            bc,
            ec,
            glyphs,
            lig_kerns,
            kerns,
            extensibles,
            params,
            bchar,
            false_bchar,
            bchar_label: (bch_label < nl).then(|| u16::try_from(bch_label).unwrap_or(0)),
        })
    }

    /// The metrics of character `c`, if the font has it (§554's
    /// `char_exists`).
    #[inline]
    #[must_use]
    pub fn glyph(&self, c: i32) -> Option<Glyph> {
        let i = usize::try_from(c.wrapping_sub(self.bc)).ok()?;
        self.glyphs.get(i).copied().flatten()
    }

    /// The metrics of character `c`, to change (pdfTeX's `\tagcode`
    /// removes tags).
    pub fn glyph_mut(&mut self, c: i32) -> Option<&mut Glyph> {
        let i = usize::try_from(c.wrapping_sub(self.bc)).ok()?;
        self.glyphs.get_mut(i).and_then(Option::as_mut)
    }

    /// pdfTeX's `auto_expand_font`: these metrics expanded by `e`
    /// thousandths (widths, italic corrections and kerns scale; heights,
    /// depths and parameters stay).
    #[must_use]
    pub fn expanded(&self, e: i32) -> Font {
        let s = |x| crate::scaled::round_xn_over_d(x, 1000 + e, 1000);
        let mut f = self.clone();
        for g in f.glyphs.iter_mut().flatten() {
            g.width = s(g.width);
            g.italic = s(g.italic);
        }
        for k in &mut f.kerns {
            *k = s(*k);
        }
        f
    }

    /// pdfTeX's `letter_space_font`: every character `d` wider.
    pub fn widen(&mut self, d: Scaled) {
        for g in self.glyphs.iter_mut().flatten() {
            g.width += d;
        }
    }

    /// pdfTeX's `get_kern`: the kern the lig/kern program of `lc` puts
    /// before `rc` (0 if none).
    #[must_use]
    pub fn kern_between(&self, lc: u8, rc: u8) -> Scaled {
        let Some(Tag::Lig(start)) = self.glyph(i32::from(lc)).map(|g| g.tag) else {
            return 0;
        };
        let mut k = usize::from(start);
        let Some(&first) = self.lig_kerns.get(k) else {
            return 0;
        };
        if first.skip > LigKern::STOP {
            k = 256 * usize::from(first.op) + usize::from(first.remainder);
        }
        while let Some(&j) = self.lig_kerns.get(k) {
            if j.next == rc && j.skip <= LigKern::STOP && j.op >= LigKern::KERN {
                let i = 256 * usize::from(j.op - LigKern::KERN) + usize::from(j.remainder);
                return self.kerns.get(i).copied().unwrap_or(0);
            }
            if j.skip == 0 {
                k += 1;
            } else {
                if j.skip >= LigKern::STOP {
                    return 0;
                }
                k += usize::from(j.skip) + 1;
            }
        }
        0
    }

    /// The width of character `c` (0 if the font lacks it).
    #[inline]
    #[must_use]
    pub fn width(&self, c: u8) -> Scaled {
        self.glyph(i32::from(c)).map_or(0, |g| g.width)
    }

    /// The total width of characters `cs`.
    #[must_use]
    pub fn run_width(&self, cs: &[u8]) -> Scaled {
        cs.iter().map(|&c| self.width(c)).sum()
    }

    /// `\fontdimen n` (0 beyond the parameters).
    #[must_use]
    pub fn param(&self, n: usize) -> Scaled {
        n.checked_sub(1)
            .and_then(|i| self.params.get(i))
            .copied()
            .unwrap_or(0)
    }
}
