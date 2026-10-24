//! TeX font metrics: TFM files (tex.web §539–§582) and virtual fonts (VF),
//! parsed into values. No file lookup: the bytes are given.
//!
//! [`parse`] validates a TFM file exactly as `read_font_info` does
//! (§560–§575) and keeps its dimensions as the file's `fix_word`s;
//! [`Tfm::at`] scales them to a size as `store_scaled` does (§571–§572),
//! so the results are TeX's to the scaled point.

// tex.web's octal constants are kept so that the code reads against the
// cited sections.
#![allow(
    clippy::unreadable_literal,
    clippy::many_single_char_names,
    clippy::too_many_lines
)]

pub mod vf;

pub use vf::{Vf, VfChar, VfError, VfFont, parse_vf};

/// A dimension in scaled points, `2^-16` pt (§101).
pub type Scaled = i32;

/// `2^16`, represents 1.0 (§101).
pub const UNITY: Scaled = 0o200000;

/// Why a TFM file is rejected. TeX reports every case alike, as
/// "Bad metric (TFM) file" (§561); the variant says which `abort` of
/// `read_font_info` was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TfmError {
    /// The file ends before the data it declares (§564, §575).
    Truncated,
    /// A size field is 32768 or more (`read_sixteen`, §564).
    SixteenBitOverflow,
    /// `bc>ec+1` or `ec>255` (§565).
    BadCharRange,
    /// `lf` disagrees with the sum of the other sizes (§565).
    BadLength,
    /// One of `nw`, `nh`, `nd`, `ni` is zero (§565).
    EmptyDimensionTable,
    /// The header has fewer than two words (§568).
    ShortHeader,
    /// The design size is negative or below 1pt (§568).
    BadDesignSize,
    /// A char info word indexes past its table (§566).
    BadCharInfo,
    /// A `list_tag` chain is circular (§570).
    CharListCycle,
    /// A `fix_word`'s first byte is neither 0 nor 255 (§571).
    BadFixWord,
    /// `width[0]`, `height[0]`, `depth[0]` or `italic[0]` is not zero (§571).
    NonzeroFirstDimension,
    /// A lig/kern step is malformed (§573).
    BadLigKern,
    /// A referenced character doesn't exist (`check_existence`, §573).
    MissingChar,
    /// The size asked for is not positive or not below 2048pt (§560).
    BadSize,
}

impl std::fmt::Display for TfmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // §561
        write!(f, "Bad metric (TFM) file ({self:?})")
    }
}

impl std::error::Error for TfmError {}

/// What a character's `remainder` means (§544).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    /// `no_tag`.
    None,
    /// `lig_tag`: `remainder` starts a lig/kern program.
    Lig,
    /// `list_tag`: `remainder` is the next larger character.
    List,
    /// `ext_tag`: `remainder` indexes the extensible recipes.
    Ext,
}

/// A `char_info_word` (§543).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CharInfo {
    /// Index into the widths; 0 means the character doesn't exist.
    pub width: u8,
    /// Index into the heights.
    pub height: u8,
    /// Index into the depths.
    pub depth: u8,
    /// Index into the italic corrections.
    pub italic: u8,
    /// The tag.
    pub tag: Tag,
    /// The remainder.
    pub remainder: u8,
}

impl CharInfo {
    /// `char_exists` (§554).
    #[must_use]
    pub fn exists(&self) -> bool {
        self.width > 0
    }
}

/// One `lig_kern_command` (§545).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LigKernStep {
    /// `skip_byte`.
    pub skip: u8,
    /// `next_char`.
    pub next: u8,
    /// `op_byte`.
    pub op: u8,
    /// `rem_byte`.
    pub remainder: u8,
}

/// `stop_flag` and `kern_flag` (§545).
pub const STOP_FLAG: u8 = 128;

impl LigKernStep {
    /// A kern step: `op_byte>=kern_flag` (§545).
    #[must_use]
    pub fn is_kern(&self) -> bool {
        self.op >= STOP_FLAG
    }

    /// A kern step's index into the kerns, `256*(op_byte-128)+remainder`
    /// (§557).
    #[must_use]
    pub fn kern_index(&self) -> usize {
        256 * usize::from(self.op.wrapping_sub(STOP_FLAG)) + usize::from(self.remainder)
    }

    /// The location an indirect step (`skip_byte>128`) points to,
    /// `256*op_byte+remainder` (§545).
    #[must_use]
    pub fn target(&self) -> usize {
        256 * usize::from(self.op) + usize::from(self.remainder)
    }
}

/// An extensible recipe (§546); 0 marks an absent piece in `top`, `mid`
/// and `bot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Exten {
    /// Top piece.
    pub top: u8,
    /// Middle piece.
    pub mid: u8,
    /// Bottom piece.
    pub bot: u8,
    /// Repeated piece.
    pub rep: u8,
}

/// A parsed TFM file. Dimensions stay `fix_word`s (four raw bytes) until
/// [`Tfm::at`] scales them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tfm {
    /// `font_check` (§568).
    pub checksum: [u8; 4],
    /// `font_dsize` in scaled points (§568).
    pub design_size: Scaled,
    /// The header's coding scheme (BCPL string, header words 2–11), if
    /// present.
    pub coding_scheme: Option<Vec<u8>>,
    /// The header's family (BCPL string, header words 12–16), if present.
    pub family: Option<Vec<u8>>,
    /// The smallest character code (`font_bc`).
    pub bc: u16,
    /// The largest character code (`font_ec`); below `bc` for an empty font.
    pub ec: u16,
    chars: Vec<CharInfo>,
    widths: Vec<[u8; 4]>,
    heights: Vec<[u8; 4]>,
    depths: Vec<[u8; 4]>,
    italics: Vec<[u8; 4]>,
    lig_kern: Vec<LigKernStep>,
    kerns: Vec<[u8; 4]>,
    exten: Vec<Exten>,
    np: usize,
    /// `param[1]`, the slant, already a scaled pure number (§575).
    slant: Scaled,
    /// `param[2..=np]` as `fix_word`s.
    params: Vec<[u8; 4]>,
    bchar: Option<u8>,
    bchar_label: Option<usize>,
    /// The lig/kern step for each pair of characters, made once: per left
    /// character with a program its row's offset in `pairs`, or
    /// `u32::MAX`; a row is 256 cells, the matching step's index plus
    /// one (0: none), as [`Tfm::lookup`] walks the program.
    rows: Vec<u32>,
    pairs: Vec<u16>,
}

/// A TFM file read byte by byte; reading past the end is `abort` (§564).
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, TfmError> {
        let b = *self.bytes.get(self.pos).ok_or(TfmError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }

    /// `read_sixteen` (§564).
    fn sixteen(&mut self) -> Result<u16, TfmError> {
        let hi = self.byte()?;
        if hi > 127 {
            return Err(TfmError::SixteenBitOverflow);
        }
        Ok(u16::from(hi) * 256 + u16::from(self.byte()?))
    }

    /// `store_four_quarters` (§564).
    fn four(&mut self) -> Result<[u8; 4], TfmError> {
        Ok([self.byte()?, self.byte()?, self.byte()?, self.byte()?])
    }

    /// A `fix_word`, checked as `store_scaled` checks it (§571).
    fn fix(&mut self) -> Result<[u8; 4], TfmError> {
        let w = self.four()?;
        if w[0] != 0 && w[0] != 255 {
            return Err(TfmError::BadFixWord);
        }
        Ok(w)
    }
}

/// A BCPL string of fewer than `max` bytes at the start of `words`.
fn bcpl(words: &[u8], max: usize) -> Option<Vec<u8>> {
    let len = usize::from(*words.first()?);
    if len >= max {
        return None;
    }
    words.get(1..=len).map(<[u8]>::to_vec)
}

/// Reads and checks a TFM file as `read_font_info` does (§560–§576).
pub fn parse(bytes: &[u8]) -> Result<Tfm, TfmError> {
    let mut r = Reader { bytes, pos: 0 };
    // §565
    let lf = usize::from(r.sixteen()?);
    let lh = usize::from(r.sixteen()?);
    let mut bc = r.sixteen()?;
    let mut ec = r.sixteen()?;
    if bc > ec + 1 || ec > 255 {
        return Err(TfmError::BadCharRange);
    }
    if bc > 255 {
        bc = 1;
        ec = 0;
    }
    let mut sizes = [0usize; 8];
    for s in &mut sizes {
        *s = usize::from(r.sixteen()?);
    }
    let [nw, nh, nd, ni, nl, nk, ne, np] = sizes;
    let nc = usize::from(ec + 1 - bc);
    if lf != 6 + lh + nc + nw + nh + nd + ni + nl + nk + ne + np {
        return Err(TfmError::BadLength);
    }
    if nw == 0 || nh == 0 || nd == 0 || ni == 0 {
        return Err(TfmError::EmptyDimensionTable);
    }
    // §568: only the first two header words matter to TeX
    if lh < 2 {
        return Err(TfmError::ShortHeader);
    }
    let checksum = r.four()?;
    let ds = r.four()?;
    if ds[0] > 127 {
        return Err(TfmError::BadDesignSize);
    }
    let design_size = ((i32::from(ds[0]) * 256 + i32::from(ds[1])) * 256 + i32::from(ds[2])) * 0o20
        + i32::from(ds[3]) / 0o20;
    if design_size < UNITY {
        return Err(TfmError::BadDesignSize);
    }
    let header_start = r.pos;
    for _ in 2..lh {
        r.four()?;
    }
    let header = &bytes[header_start..r.pos];
    let coding_scheme = if lh >= 12 {
        bcpl(&header[..40], 40)
    } else {
        None
    };
    let family = if lh >= 17 {
        bcpl(&header[40..60], 20)
    } else {
        None
    };
    // §566
    let mut chars: Vec<CharInfo> = Vec::with_capacity(nc);
    for k in 0..nc {
        let [a, b, c, d] = r.four()?;
        if usize::from(a) >= nw
            || usize::from(b / 0o20) >= nh
            || usize::from(b % 0o20) >= nd
            || usize::from(c / 4) >= ni
        {
            return Err(TfmError::BadCharInfo);
        }
        let tag = match c % 4 {
            1 => {
                if usize::from(d) >= nl {
                    return Err(TfmError::BadCharInfo);
                }
                Tag::Lig
            }
            3 => {
                if usize::from(d) >= ne {
                    return Err(TfmError::BadCharInfo);
                }
                Tag::Ext
            }
            2 => {
                // §570: a cycle is found at its largest code
                let current = k + usize::from(bc);
                let mut d = usize::from(d);
                if d < usize::from(bc) || d > usize::from(ec) {
                    return Err(TfmError::BadCharInfo);
                }
                while d < current {
                    let q = chars[d - usize::from(bc)];
                    if q.tag != Tag::List {
                        break;
                    }
                    d = usize::from(q.remainder);
                }
                if d == current {
                    return Err(TfmError::CharListCycle);
                }
                Tag::List
            }
            _ => Tag::None,
        };
        chars.push(CharInfo {
            width: a,
            height: b / 0o20,
            depth: b % 0o20,
            italic: c / 4,
            tag,
            remainder: d,
        });
    }
    // §571; that the first entries are zero is checked at a size, by `at`
    let mut table =
        |n: usize| -> Result<Vec<[u8; 4]>, TfmError> { (0..n).map(|_| r.fix()).collect() };
    let widths = table(nw)?;
    let heights = table(nh)?;
    let depths = table(nd)?;
    let italics = table(ni)?;
    // §573 `check_existence`
    let exists = |c: u8| -> Result<(), TfmError> {
        if u16::from(c) < bc || u16::from(c) > ec {
            return Err(TfmError::MissingChar);
        }
        if chars[usize::from(c) - usize::from(bc)].exists() {
            Ok(())
        } else {
            Err(TfmError::MissingChar)
        }
    };
    // §573
    let mut bch_label = 0o77777usize;
    let mut bchar: u16 = 256;
    let mut lig_kern = Vec::with_capacity(nl);
    for k in 0..nl {
        let [a, b, c, d] = r.four()?;
        let step = LigKernStep {
            skip: a,
            next: b,
            op: c,
            remainder: d,
        };
        if a > 128 {
            if step.target() >= nl {
                return Err(TfmError::BadLigKern);
            }
            if a == 255 && k == 0 {
                bchar = u16::from(b);
            }
        } else {
            if u16::from(b) != bchar {
                exists(b)?;
            }
            if c < 128 {
                exists(d)?;
            } else if step.kern_index() >= nk {
                return Err(TfmError::BadLigKern);
            }
            if a < 128 && k + usize::from(a) + 1 >= nl {
                return Err(TfmError::BadLigKern);
            }
        }
        if k + 1 == nl && a == 255 {
            bch_label = step.target();
        }
        lig_kern.push(step);
    }
    let kerns = (0..nk).map(|_| r.fix()).collect::<Result<Vec<_>, _>>()?;
    // §574
    let mut exten = Vec::with_capacity(ne);
    for _ in 0..ne {
        let [a, b, c, d] = r.four()?;
        for piece in [a, b, c] {
            if piece != 0 {
                exists(piece)?;
            }
        }
        exists(d)?;
        exten.push(Exten {
            top: a,
            mid: b,
            bot: c,
            rep: d,
        });
    }
    // §575: the slant is a pure number; the file must not end early
    let mut slant = 0;
    let mut params = Vec::with_capacity(np.saturating_sub(1));
    for k in 1..=np {
        if k == 1 {
            let [a, b, c, d] = r.four()?;
            let mut sw = i32::from(a);
            if sw > 127 {
                sw -= 256;
            }
            sw = (sw * 0o400 + i32::from(b)) * 0o400 + i32::from(c);
            slant = sw * 0o20 + i32::from(d) / 0o20;
        } else {
            params.push(r.fix()?);
        }
    }
    // §576
    Ok(Tfm {
        checksum,
        design_size,
        coding_scheme,
        family,
        bc,
        ec,
        chars,
        widths,
        heights,
        depths,
        italics,
        lig_kern,
        kerns,
        exten,
        np,
        slant,
        params,
        bchar: u8::try_from(bchar).ok(),
        bchar_label: (bch_label < nl).then_some(bch_label),
        rows: Vec::new(),
        pairs: Vec::new(),
    }
    .with_pairs())
}

/// `xn_over_d(x, n, d)` (§107) for nonnegative `n` and positive `d`.
fn xn_over_d(x: Scaled, n: i32, d: i32) -> Scaled {
    let q = i64::from(x).abs() * i64::from(n) / i64::from(d);
    let q = i32::try_from(q).unwrap_or(i32::MAX);
    if x >= 0 { q } else { -q }
}

/// The `z'`, `alpha` and `beta` of §572 for size `z`.
fn scaler(mut z: Scaled) -> (Scaled, i32, i32) {
    let mut alpha = 16;
    while z >= 0o40000000 {
        z /= 2;
        alpha += alpha;
    }
    let beta = 256 / alpha;
    (z, alpha * z, beta)
}

/// `store_scaled` (§571): a `fix_word` times `z`, exactly.
fn store_scaled(w: [u8; 4], (z, alpha, beta): (Scaled, i32, i32)) -> Scaled {
    let [a, b, c, d] = w.map(i32::from);
    let sw = (((d * z) / 0o400 + c * z) / 0o400 + b * z) / beta;
    if a == 0 { sw } else { sw - alpha }
}

/// A character's scaled metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CharDims {
    /// `char_width`.
    pub width: Scaled,
    /// `char_height`.
    pub height: Scaled,
    /// `char_depth`.
    pub depth: Scaled,
    /// `char_italic`.
    pub italic: Scaled,
}

/// A TFM file's metrics at one size (`font_size`), as `read_font_info`
/// stores them (§571–§576).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScaledFont {
    /// `font_size`.
    pub size: Scaled,
    /// `font_dsize`.
    pub design_size: Scaled,
    /// Indexed by character code (256 entries); `None` where no
    /// character exists.
    pub chars: Vec<Option<CharDims>>,
    /// The kerns (`char_kern`, §557), by kern index.
    pub kerns: Vec<Scaled>,
    /// `param(1..=font_params)`, at least seven (§575–§576); `params[0]`
    /// is the slant, unscaled.
    pub params: Vec<Scaled>,
}

impl ScaledFont {
    /// `param(n)` (§558), 1-based; 0 past `font_params`.
    #[must_use]
    pub fn param(&self, n: usize) -> Scaled {
        n.checked_sub(1)
            .and_then(|i| self.params.get(i))
            .copied()
            .unwrap_or(0)
    }

    /// `slant` (§558).
    #[must_use]
    pub fn slant(&self) -> Scaled {
        self.param(1)
    }
    /// `space` (§558).
    #[must_use]
    pub fn space(&self) -> Scaled {
        self.param(2)
    }
    /// `space_stretch` (§558).
    #[must_use]
    pub fn space_stretch(&self) -> Scaled {
        self.param(3)
    }
    /// `space_shrink` (§558).
    #[must_use]
    pub fn space_shrink(&self) -> Scaled {
        self.param(4)
    }
    /// `x_height` (§558).
    #[must_use]
    pub fn x_height(&self) -> Scaled {
        self.param(5)
    }
    /// `quad` (§558).
    #[must_use]
    pub fn quad(&self) -> Scaled {
        self.param(6)
    }
    /// `extra_space` (§558).
    #[must_use]
    pub fn extra_space(&self) -> Scaled {
        self.param(7)
    }

    /// The metrics of `c`, if it exists.
    #[must_use]
    pub fn char(&self, c: u8) -> Option<CharDims> {
        self.chars.get(usize::from(c)).copied().flatten()
    }

    /// The width of `c`, if it exists.
    #[must_use]
    pub fn width(&self, c: u8) -> Option<Scaled> {
        self.char(c).map(|d| d.width)
    }

    /// The kern a kern step inserts (`char_kern`, §557).
    #[must_use]
    pub fn char_kern(&self, step: LigKernStep) -> Scaled {
        self.kerns.get(step.kern_index()).copied().unwrap_or(0)
    }
}

impl Tfm {
    /// The size `read_font_info`'s `s` asks for (§568): `s>=0` is an
    /// `at` size, `s<0` is `scaled -s` (−1000: the design size).
    #[must_use]
    pub fn size_for(&self, s: i32) -> Scaled {
        if s >= 0 {
            s
        } else {
            xn_over_d(self.design_size, -s, 1000)
        }
    }

    /// The metrics at size `z` (§568, §571–§576).
    ///
    /// # Errors
    /// [`TfmError::BadSize`] unless `0 < z < 2^27` (§560);
    /// [`TfmError::NonzeroFirstDimension`] when the first width, height,
    /// depth or italic correction doesn't scale to zero (§571).
    pub fn at(&self, z: Scaled) -> Result<ScaledFont, TfmError> {
        if z <= 0 || z >= 0o1000000000 {
            return Err(TfmError::BadSize);
        }
        let s = scaler(z);
        let scale =
            |t: &[[u8; 4]]| -> Vec<Scaled> { t.iter().map(|&w| store_scaled(w, s)).collect() };
        let (w, h, d, i) = (
            scale(&self.widths),
            scale(&self.heights),
            scale(&self.depths),
            scale(&self.italics),
        );
        if w[0] != 0 || h[0] != 0 || d[0] != 0 || i[0] != 0 {
            return Err(TfmError::NonzeroFirstDimension);
        }
        let mut chars = vec![None; 256];
        for (k, ci) in self.chars.iter().enumerate() {
            if ci.exists() {
                chars[k + usize::from(self.bc)] = Some(CharDims {
                    width: w[usize::from(ci.width)],
                    height: h[usize::from(ci.height)],
                    depth: d[usize::from(ci.depth)],
                    italic: i[usize::from(ci.italic)],
                });
            }
        }
        // §575: parameters past `np`, up to seven, are zero
        let mut params = Vec::with_capacity(self.font_params());
        if self.np > 0 {
            params.push(self.slant);
        }
        params.extend(self.params.iter().map(|&p| store_scaled(p, s)));
        params.resize(self.font_params(), 0);
        Ok(ScaledFont {
            size: z,
            design_size: self.design_size,
            chars,
            kerns: scale(&self.kerns),
            params,
        })
    }

    /// `char_info(c)` (§554), if `c` is in `bc..=ec`.
    #[must_use]
    pub fn char_info(&self, c: u8) -> Option<CharInfo> {
        let k = u16::from(c).checked_sub(self.bc)?;
        self.chars.get(usize::from(k)).copied()
    }

    /// Whether `c` exists (`char_exists`, §554).
    #[must_use]
    pub fn exists(&self, c: u8) -> bool {
        self.char_info(c).is_some_and(|i| i.exists())
    }

    /// The whole lig/kern array (§545).
    #[must_use]
    pub fn lig_kern_array(&self) -> &[LigKernStep] {
        &self.lig_kern
    }

    /// The extensible recipes (§546).
    #[must_use]
    pub fn extens(&self) -> &[Exten] {
        &self.exten
    }

    /// The number of parameters in the file (`np`).
    #[must_use]
    pub fn param_count(&self) -> usize {
        self.np
    }

    /// `font_params` (§576): `np`, at least 7.
    #[must_use]
    pub fn font_params(&self) -> usize {
        self.np.max(7)
    }

    /// `font_bchar` (§576): the right boundary character, if any.
    #[must_use]
    pub fn bchar(&self) -> Option<u8> {
        self.bchar
    }

    /// `font_false_bchar` (§576): the boundary character when no real
    /// character has its code, else none.
    #[must_use]
    pub fn false_bchar(&self) -> Option<u8> {
        self.bchar.filter(|&b| !self.exists(b))
    }

    /// `bchar_label` (§576): where the left boundary's program starts.
    #[must_use]
    pub fn bchar_label(&self) -> Option<usize> {
        self.bchar_label
    }

    /// The lig/kern program of `left` (`None`: the left boundary), as
    /// §1039 walks it: from `lig_kern_start`, or `lig_kern_restart` when
    /// the first step is indirect (`skip_byte>stop_flag`, §557), or from
    /// `bchar_label` for the boundary (§1040); ending after a step whose
    /// `skip_byte>=stop_flag`. Each item is the step's location and the
    /// step.
    #[must_use]
    pub fn program(&self, left: Option<u8>) -> Program<'_> {
        let start = match left {
            None => self.bchar_label,
            Some(c) => self.char_info(c).filter(|i| i.tag == Tag::Lig).map(|i| {
                // §1039
                let k = usize::from(i.remainder);
                let j = self.lig_kern[k];
                if j.skip > STOP_FLAG { j.target() } else { k }
            }),
        };
        Program {
            steps: &self.lig_kern,
            next: start,
        }
    }

    /// The step that applies to `left` followed by `right` (§1039), or
    /// `None` (`goto main_loop_wrapup`).
    ///
    /// `left: None` is the left boundary (`cur_l=non_char`; a ligature
    /// matched there sets `lft_hit`, §1040). `right: None` is the right
    /// boundary, `cur_r:=bchar` (§1036–§1038); a ligature matched there
    /// sets `rt_hit` (§1040), and with no boundary character in the font
    /// nothing matches. A step with `skip_byte>stop_flag` met while
    /// walking is a halt, never a match (§545). The caller maps a right
    /// character equal to [`Tfm::false_bchar`] to no match, as §1038 does.
    #[must_use]
    pub fn lookup(&self, left: Option<u8>, right: Option<u8>) -> Option<LigKernStep> {
        let cur_r = right.or(self.bchar)?;
        if let Some(c) = left {
            let row = *self.rows.get(usize::from(c))?;
            if row == u32::MAX {
                return None;
            }
            let k = self.pairs[row as usize + usize::from(cur_r)];
            return (k != 0).then(|| self.lig_kern[usize::from(k - 1)]);
        }
        self.program(left)
            .map(|(_, j)| j)
            .find(|j| j.next == cur_r && j.skip <= STOP_FLAG)
    }

    /// The font without its characters' lig/kern programs
    /// (`set_no_ligatures`, pdfTeX: a character's `lig_tag` taken off,
    /// its kerns gone with its ligatures; the boundary's program kept).
    #[must_use]
    pub fn without_ligatures(mut self) -> Tfm {
        for ci in &mut self.chars {
            if ci.exists() && ci.tag == Tag::Lig {
                ci.tag = Tag::None;
            }
        }
        self.with_pairs()
    }

    /// The font with its pair table ([`Tfm::lookup`]): each left
    /// character's program walked once, the first step for each right
    /// character kept.
    fn with_pairs(mut self) -> Tfm {
        let mut rows = vec![u32::MAX; 256];
        let mut pairs: Vec<u16> = Vec::new();
        for c in 0..=255u8 {
            let mut prog = self.program(Some(c)).peekable();
            if prog.peek().is_none() {
                continue;
            }
            let off = pairs.len();
            pairs.resize(off + 256, 0);
            for (k, j) in prog {
                let cell = &mut pairs[off + usize::from(j.next)];
                if *cell == 0 && j.skip <= STOP_FLAG {
                    *cell = u16::try_from(k + 1).unwrap_or(0);
                }
            }
            rows[usize::from(c)] = u32::try_from(off).unwrap_or(u32::MAX);
        }
        self.rows = rows;
        self.pairs = pairs;
        self
    }
}

/// A lig/kern program, walked as §1039 walks it.
#[derive(Clone, Debug)]
pub struct Program<'a> {
    steps: &'a [LigKernStep],
    next: Option<usize>,
}

impl Iterator for Program<'_> {
    type Item = (usize, LigKernStep);

    fn next(&mut self) -> Option<Self::Item> {
        let k = self.next?;
        let j = *self.steps.get(k)?;
        // §1039
        self.next = match j.skip {
            0 => Some(k + 1),
            s if s >= STOP_FLAG => None,
            s => Some(k + usize::from(s) + 1),
        };
        Some((k, j))
    }
}
