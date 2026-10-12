//! Virtual fonts: the VF format as `vftovp.web` reads it (its §6–§11 on
//! the format, §30–§46 on reading it): the preamble, the local fonts and
//! each character's DVI program, kept as byte slices of the file.
//!
//! Checks that need the virtual font's own TFM file (that each character
//! exists, its TFM width) are the caller's.

#![allow(clippy::many_single_char_names)]

/// `id_byte` (vftovp §7).
const ID_BYTE: u8 = 202;
/// `long_char` (vftovp §8).
const LONG_CHAR: u8 = 242;
/// `fnt_def1` (vftovp §8).
const FNT_DEF1: u8 = 243;
/// `pre` (vftovp §8).
const PRE: u8 = 247;
/// `post` (vftovp §8).
const POST: u8 = 248;

/// Why a VF file is rejected: `vftovp`'s `vf_abort` cases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfError {
    /// "The first byte isn't \`pre'!"
    NoPreamble,
    /// "Wrong VF version number in second byte!"
    WrongId,
    /// "The file ended prematurely!"
    Truncated,
    /// "Illegal byte ... at beginning of character packet!"
    IllegalByte(u8),
    /// "Mapped font size is too big!"
    SizeTooBig,
    /// "Negative packet length!"
    NegativeLength,
}

impl std::fmt::Display for VfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VfError::NoPreamble => write!(f, "The first byte isn't `pre'!"),
            VfError::WrongId => write!(f, "Wrong VF version number in second byte!"),
            VfError::Truncated => write!(f, "The file ended prematurely!"),
            VfError::IllegalByte(b) => {
                write!(f, "Illegal byte {b} at beginning of character packet!")
            }
            VfError::SizeTooBig => write!(f, "Mapped font size is too big!"),
            VfError::NegativeLength => write!(f, "Negative packet length!"),
        }
    }
}

impl std::error::Error for VfError {}

/// A local font (`fnt_def`, vftovp §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VfFont {
    /// The font number `k`.
    pub number: u32,
    /// Checksum `c[4]`.
    pub checksum: [u8; 4],
    /// Scaled size `s[4]`, a `fix_word` relative to the virtual font's
    /// design size.
    pub scaled: i32,
    /// Design size `d[4]`, a `fix_word` in points (`2^-20` units).
    pub design_size: i32,
    /// Area `n[0..a]`.
    pub area: Vec<u8>,
    /// Name `n[a..a+l]`.
    pub name: Vec<u8>,
}

/// A character packet (vftovp §8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VfChar<'a> {
    /// Character code `cc`.
    pub code: u32,
    /// Its TFM width `tfm`, a `fix_word`.
    pub tfm_width: i32,
    /// Its DVI program `dvi[pl]`.
    pub dvi: &'a [u8],
}

/// A parsed VF file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vf<'a> {
    /// The preamble's comment.
    pub comment: &'a [u8],
    /// The preamble's checksum `cs[4]`.
    pub checksum: [u8; 4],
    /// The preamble's design size `ds[4]`, a `fix_word`.
    pub design_size: i32,
    /// The local fonts, in file order.
    pub fonts: Vec<VfFont>,
    /// The character packets, in file order; a later packet for a code
    /// replaces the earlier one ("Discarding earlier packet", vftovp §46).
    pub chars: Vec<VfChar<'a>>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], VfError> {
        let end = self.pos.checked_add(n).ok_or(VfError::Truncated)?;
        let s = self.bytes.get(self.pos..end).ok_or(VfError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn byte(&mut self) -> Result<u8, VfError> {
        Ok(self.take(1)?[0])
    }

    /// `vf_read(k)` (vftovp §45): `k` bytes, big-endian, the first one
    /// signed when `k=4`.
    fn int(&mut self, k: usize) -> Result<i32, VfError> {
        let s = self.take(k)?;
        let mut x = if k == 4 {
            i32::from(s[0].cast_signed())
        } else {
            i32::from(s[0])
        };
        for &b in &s[1..] {
            x = x.wrapping_mul(256).wrapping_add(i32::from(b));
        }
        Ok(x)
    }

    fn four(&mut self) -> Result<[u8; 4], VfError> {
        let s = self.take(4)?;
        Ok([s[0], s[1], s[2], s[3]])
    }
}

/// Reads a VF file as `vftovp` does (vftovp §30–§46).
pub fn parse_vf(bytes: &[u8]) -> Result<Vf<'_>, VfError> {
    let mut r = Reader { bytes, pos: 0 };
    // vftovp §32: the preamble
    if r.byte().map_err(|_| VfError::NoPreamble)? != PRE {
        return Err(VfError::NoPreamble);
    }
    if r.byte()? != ID_BYTE {
        return Err(VfError::WrongId);
    }
    let k = usize::from(r.byte()?);
    let comment = r.take(k)?;
    let checksum = r.four()?;
    let design_size = r.int(4)?;
    let mut vf = Vf {
        comment,
        checksum,
        design_size,
        fonts: Vec::new(),
        chars: Vec::new(),
    };
    // vftovp §33: font definitions, then character packets, then `post`
    let mut packet_found = false;
    // "File ended without a postamble!" is only a warning
    while let Ok(b) = r.byte() {
        if b == POST {
            break;
        }
        if b > LONG_CHAR {
            // vftovp §35
            if packet_found || b >= PRE {
                return Err(VfError::IllegalByte(b));
            }
            let number = r.int(usize::from(b - FNT_DEF1) + 1)?;
            let checksum = r.four()?;
            let s = r.four()?;
            if s[0] > 0 {
                return Err(VfError::SizeTooBig);
            }
            let scaled = i32::from_be_bytes(s);
            let design_size = r.int(4)?;
            let a = usize::from(r.byte()?);
            let l = usize::from(r.byte()?);
            let area = r.take(a)?.to_vec();
            let name = r.take(l)?.to_vec();
            vf.fonts.push(VfFont {
                number: number.cast_unsigned(),
                checksum,
                scaled,
                design_size,
                area,
                name,
            });
        } else {
            // vftovp §46
            let (pl, code, tfm_width) = if b == LONG_CHAR {
                (r.int(4)?, r.int(4)?, r.int(4)?)
            } else {
                (i32::from(b), r.int(1)?, r.int(3)?)
            };
            let pl = usize::try_from(pl).map_err(|_| VfError::NegativeLength)?;
            let dvi = r.take(pl)?;
            let ch = VfChar {
                code: code.cast_unsigned(),
                tfm_width,
                dvi,
            };
            if let Some(old) = vf.chars.iter_mut().find(|c| c.code == ch.code) {
                *old = ch;
            } else {
                vf.chars.push(ch);
            }
            packet_found = true;
        }
    }
    Ok(vf)
}
