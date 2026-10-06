//! A PDF image as the browser draws it: a `data:` URI, JPEG or PNG. A
//! JPEG (`DCTDecode`) passes through; a `FlateDecode` stream with PNG
//! predictors in a colour space PNG has is wrapped as a PNG as it is;
//! anything else is decoded to samples (`pdfread`'s filters) and encoded
//! as a PNG (`miniz_oxide`), its soft mask (`/SMask`) as its alpha, an
//! image mask (`/ImageMask`) in the fill colour.

// (samples and sizes: integers of known ranges)
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]

use std::fmt::Write as _;

/// An image's colour space, as its samples read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// A palette of RGB colours (`/Indexed`, its base made RGB).
    Indexed(Vec<[u8; 3]>),
}

impl Space {
    /// Components a sample has.
    pub(crate) fn n(&self) -> usize {
        match self {
            Space::Gray | Space::Indexed(_) => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
        }
    }
}

/// What an image is made of, read from its dictionary.
pub(crate) struct Image {
    pub width: usize,
    pub height: usize,
    pub bpc: usize,
    pub space: Space,
    /// `/Decode`, if given (a component's range).
    pub decode: Option<Vec<f64>>,
    /// `/ImageMask true`: one bit a sample, painted in `fill` where 0
    /// (where 1 with `/Decode [1 0]`).
    pub mask: bool,
    /// The encoded bytes, and their filters (outermost first).
    pub raw: Vec<u8>,
    pub filters: Vec<String>,
    /// `FlateDecode`'s `/Predictor`, `/Colors`, `/Columns`, `/BitsPerComponent`.
    pub predictor: (i64, i64, i64, i64),
    /// The samples, decoded through every filter (`pdfread`'s), if they
    /// can be.
    pub samples: Option<Vec<u8>>,
}

/// The data URI of `img`, its alpha from `smask` (a gray image of any
/// size), an image mask filled with `fill`. `None`: one it cannot draw
/// (JPEG 2000, JBIG2, CCITT; a colour space it does not know).
pub(crate) fn data_uri(img: &Image, smask: Option<&Image>, fill: [u8; 3]) -> Option<String> {
    let only = |f: &str| img.filters.len() == 1 && img.filters[0] == f;
    if img.mask {
        let s = img.samples.as_ref()?;
        let on = img.decode.as_ref().is_some_and(|d| d.first() == Some(&1.0));
        let row = img.width.div_ceil(8);
        let mut rgba = Vec::with_capacity(img.width * img.height * 4);
        for y in 0..img.height {
            for x in 0..img.width {
                let bit = s.get(y * row + x / 8).map_or(1, |b| (b >> (7 - x % 8)) & 1);
                let paint = (bit == 1) == on;
                rgba.extend_from_slice(&[fill[0], fill[1], fill[2], if paint { 255 } else { 0 }]);
            }
        }
        return Some(uri("png", &png(img.width, img.height, 6, &rgba, None)));
    }
    if smask.is_none() && only("DCTDecode") {
        return Some(uri("jpeg", &img.raw));
    }
    // (PNG's own rows: the zlib stream wrapped, not decoded)
    let (pred, colors, columns, bits) = img.predictor;
    if smask.is_none()
        && only("FlateDecode")
        && pred >= 10
        && img.decode.is_none()
        && usize::try_from(columns).ok() == Some(img.width)
        && usize::try_from(bits).ok() == Some(img.bpc)
        && usize::try_from(colors).ok() == Some(img.space.n())
        && matches!(img.bpc, 1 | 2 | 4 | 8 | 16)
    {
        let (ctype, plte) = match &img.space {
            Space::Gray => (Some(0), None),
            Space::Rgb if img.bpc >= 8 => (Some(2), None),
            Space::Indexed(p) if img.bpc <= 8 => (Some(3), Some(p.as_slice())),
            _ => (None, None),
        };
        if let Some(ctype) = ctype {
            return Some(uri(
                "png",
                &png_zlib(img.width, img.height, img.bpc, ctype, plte, &img.raw),
            ));
        }
    }
    let rgb = rgb8(img)?;
    let alpha = smask.and_then(|m| alpha8(m, img.width, img.height));
    match alpha {
        Some(a) => {
            let mut rgba = Vec::with_capacity(img.width * img.height * 4);
            for (i, px) in rgb.chunks(3).enumerate() {
                rgba.extend_from_slice(px);
                rgba.push(a.get(i).copied().unwrap_or(255));
            }
            Some(uri("png", &png(img.width, img.height, 6, &rgba, None)))
        }
        None => Some(uri("png", &png(img.width, img.height, 2, &rgb, None))),
    }
}

/// The samples of `img` unpacked to one byte a component (scaled from
/// `bpc` bits).
fn unpack(img: &Image, n: usize) -> Option<Vec<u8>> {
    let s = img.samples.as_ref()?;
    let (w, h, bpc) = (img.width, img.height, img.bpc);
    let row = (w * n * bpc).div_ceil(8);
    let mut out = Vec::with_capacity(w * h * n);
    for y in 0..h {
        let r = s.get(y * row..(y + 1) * row).unwrap_or(&[]);
        for i in 0..w * n {
            let v = match bpc {
                8 => r.get(i).copied().unwrap_or(0),
                16 => r.get(2 * i).copied().unwrap_or(0),
                1 | 2 | 4 => {
                    let bit = i * bpc;
                    let b = r.get(bit / 8).copied().unwrap_or(0);
                    let v = (b >> (8 - bpc - bit % 8)) & ((1 << bpc) - 1);
                    // (an index stays an index; a level scales to 0–255)
                    if matches!(img.space, Space::Indexed(_)) {
                        v
                    } else {
                        (u32::from(v) * 255 / ((1 << bpc) - 1)) as u8
                    }
                }
                _ => return None,
            };
            out.push(v);
        }
    }
    Some(out)
}

/// `img` as 8-bit RGB.
fn rgb8(img: &Image) -> Option<Vec<u8>> {
    let n = img.space.n();
    let mut v = unpack(img, n)?;
    // (`/Decode` on levels: each component's range mapped back)
    if let (Some(d), false) = (&img.decode, matches!(img.space, Space::Indexed(_))) {
        for (i, x) in v.iter_mut().enumerate() {
            let k = i % n;
            let (lo, hi) = (
                d.get(2 * k).copied().unwrap_or(0.0),
                d.get(2 * k + 1).copied().unwrap_or(1.0),
            );
            *x = ((lo + f64::from(*x) / 255.0 * (hi - lo)).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    Some(match &img.space {
        Space::Rgb => v,
        Space::Gray => v.iter().flat_map(|&g| [g, g, g]).collect(),
        Space::Cmyk => v.chunks(4).flat_map(cmyk).collect(),
        Space::Indexed(pal) => v
            .iter()
            .flat_map(|&i| pal.get(usize::from(i)).copied().unwrap_or([0, 0, 0]))
            .collect(),
    })
}

/// An 8-bit CMYK sample as RGB (the naive conversion).
pub(crate) fn cmyk(p: &[u8]) -> [u8; 3] {
    let k = 255 - u32::from(*p.get(3).unwrap_or(&0));
    let c = |v: u8| ((255 - u32::from(v)) * k / 255) as u8;
    [
        c(p[0]),
        c(*p.get(1).unwrap_or(&0)),
        c(*p.get(2).unwrap_or(&0)),
    ]
}

/// A soft mask's levels as alpha, resampled (nearest) to `w`×`h`.
fn alpha8(m: &Image, w: usize, h: usize) -> Option<Vec<u8>> {
    let g = unpack(m, 1)?;
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let sy = (y * m.height / h.max(1)).min(m.height.saturating_sub(1));
        for x in 0..w {
            let sx = (x * m.width / w.max(1)).min(m.width.saturating_sub(1));
            out.push(g.get(sy * m.width + sx).copied().unwrap_or(255));
        }
    }
    Some(out)
}

fn uri(kind: &str, data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 4 / 3 + 32);
    let _ = write!(s, "data:image/{kind};base64,");
    base64(data, &mut s);
    s
}

/// Standard base64, padded.
fn base64(data: &[u8], out: &mut String) {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18u32, 12, 6, 0].iter().enumerate() {
            out.push(if i <= c.len() {
                char::from(A[((n >> shift) & 63) as usize])
            } else {
                '='
            });
        }
    }
}

/// A PNG of 8-bit samples `data` (rows of `w` pixels of colour type
/// `ctype`: 0 gray, 2 RGB, 6 RGBA), each row given filter 0.
fn png(w: usize, h: usize, ctype: u8, data: &[u8], plte: Option<&[[u8; 3]]>) -> Vec<u8> {
    let n = match ctype {
        0 => 1,
        2 => 3,
        _ => 4,
    };
    let mut rows = Vec::with_capacity(h * (w * n + 1));
    for r in data.chunks(w * n).take(h) {
        rows.push(0);
        rows.extend_from_slice(r);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&rows, 6);
    png_zlib(w, h, 8, ctype, plte, &z)
}

/// A PNG of zlib stream `z` (rows with PNG's filter bytes).
fn png_zlib(
    w: usize,
    h: usize,
    bpc: usize,
    ctype: u8,
    plte: Option<&[[u8; 3]]>,
    z: &[u8],
) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[bpc as u8, ctype, 0, 0, 0]);
    chunk(&mut out, *b"IHDR", &ihdr);
    if let Some(p) = plte {
        let bytes: Vec<u8> = p.iter().flatten().copied().collect();
        chunk(&mut out, *b"PLTE", &bytes);
    }
    chunk(&mut out, *b"IDAT", z);
    chunk(&mut out, *b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let c = crc32(&out[start..]);
    out.extend_from_slice(&c.to_be_bytes());
}

/// CRC-32 (ISO 3309, PNG's).
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(space: Space, bpc: usize, samples: &[u8]) -> Image {
        Image {
            width: 2,
            height: 1,
            bpc,
            space,
            decode: None,
            mask: false,
            raw: Vec::new(),
            filters: Vec::new(),
            predictor: (1, 1, 1, 8),
            samples: Some(samples.to_vec()),
        }
    }

    #[test]
    fn crc_and_png_header() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        let p = png(1, 1, 2, &[255, 0, 0], None);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&p[12..16], b"IHDR");
        assert!(p.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn samples_to_rgb() {
        assert_eq!(
            rgb8(&img(Space::Gray, 8, &[0, 255])),
            Some(vec![0, 0, 0, 255, 255, 255])
        );
        assert_eq!(
            rgb8(&img(Space::Gray, 1, &[0b0100_0000])),
            Some(vec![0, 0, 0, 255, 255, 255])
        );
        assert_eq!(
            rgb8(&img(Space::Cmyk, 8, &[255, 0, 0, 0, 0, 0, 0, 255])),
            Some(vec![0, 255, 255, 0, 0, 0])
        );
        let pal = Space::Indexed(vec![[1, 2, 3], [4, 5, 6]]);
        assert_eq!(rgb8(&img(pal, 8, &[1, 0])), Some(vec![4, 5, 6, 1, 2, 3]));
        // (a mask: painted where 0)
        let mut m = img(Space::Gray, 1, &[0b0100_0000]);
        m.mask = true;
        let u = data_uri(&m, None, [9, 9, 9]).unwrap();
        assert!(u.starts_with("data:image/png;base64,iVBORw0KGgo"));
        // (a JPEG as it is)
        let mut j = img(Space::Rgb, 8, &[]);
        j.raw = vec![0xff, 0xd8, 0xff];
        j.filters = vec!["DCTDecode".into()];
        assert_eq!(
            data_uri(&j, None, [0; 3]).unwrap(),
            "data:image/jpeg;base64,/9j/"
        );
    }
}
