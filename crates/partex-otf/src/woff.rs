//! WOFF 1.0 to sfnt, as FreeType's `woff_open_font` unpacks it (each
//! table inflated when its compressed length is below its original
//! length), and WOFF2 to sfnt as `woff2_open_font` does (brotli, then the
//! `glyf`/`loca` and `hmtx` transforms undone).

use alloc::vec::Vec;

use crate::face::{rd_u16, rd_u32};

/// Whether `data` is a WOFF (`wOFF`) file.
#[must_use]
pub fn is_woff(data: &[u8]) -> bool {
    data.get(0..4) == Some(b"wOFF")
}

/// Whether `data` is a WOFF2 (`wOF2`) file.
#[must_use]
pub fn is_woff2(data: &[u8]) -> bool {
    data.get(0..4) == Some(b"wOF2")
}

/// The sfnt a WOFF file holds, or `None` if it is not one or is damaged.
#[must_use]
pub fn decode(data: &[u8]) -> Option<Vec<u8>> {
    if !is_woff(data) {
        return None;
    }
    let flavor = rd_u32(data, 4)?;
    let n = rd_u16(data, 12)? as usize;
    let mut tables = Vec::with_capacity(n);
    for i in 0..n {
        let r = 44 + 20 * i;
        let tag = rd_u32(data, r)?;
        let off = rd_u32(data, r + 4)? as usize;
        let comp = rd_u32(data, r + 8)? as usize;
        let orig = rd_u32(data, r + 12)? as usize;
        let csum = rd_u32(data, r + 16)?;
        let raw = data.get(off..off + comp)?;
        let bytes = match comp.cmp(&orig) {
            core::cmp::Ordering::Less => {
                let z = raw;
                if z.len() < 2 {
                    return None;
                }
                let mut out = Vec::with_capacity(orig);
                crate::inflate::inflate_raw(&z[2..], &mut out);
                if out.len() != orig {
                    return None;
                }
                out
            }
            core::cmp::Ordering::Equal => raw.to_vec(),
            core::cmp::Ordering::Greater => return None,
        };
        tables.push((tag, csum, bytes));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    let mut sr = 1u16;
    let mut es = 0u16;
    while usize::from(sr) * 2 <= n {
        sr *= 2;
        es += 1;
    }
    out.extend_from_slice(&(sr * 16).to_be_bytes());
    out.extend_from_slice(&es.to_be_bytes());
    out.extend_from_slice(&((n as u16) * 16 - sr * 16).to_be_bytes());
    let mut pos = 12 + 16 * n;
    let mut offs = Vec::with_capacity(n);
    for (_, _, b) in &tables {
        offs.push(pos);
        pos += (b.len() + 3) & !3;
    }
    for ((tag, csum, b), &o) in tables.iter().zip(&offs) {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&csum.to_be_bytes());
        out.extend_from_slice(&(o as u32).to_be_bytes());
        out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    }
    for (_, _, b) in &tables {
        out.extend_from_slice(b);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    Some(out)
}

/// Allocations for the brotli decoder, from `alloc`.
struct VecAlloc;

#[derive(Default)]
struct Mem<T>(Vec<T>);

impl<T> brotli_decompressor::SliceWrapper<T> for Mem<T> {
    fn slice(&self) -> &[T] {
        &self.0
    }
}

impl<T> brotli_decompressor::SliceWrapperMut<T> for Mem<T> {
    fn slice_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<T: Clone + Default> brotli_decompressor::Allocator<T> for VecAlloc {
    type AllocatedMemory = Mem<T>;
    fn alloc_cell(&mut self, len: usize) -> Mem<T> {
        Mem(alloc::vec![T::default(); len])
    }
    fn free_cell(&mut self, _data: Mem<T>) {}
}

/// Decompresses a brotli stream known to inflate to `size` bytes.
fn brotli(input: &[u8], size: usize) -> Option<Vec<u8>> {
    use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState};
    let mut state = BrotliState::new(VecAlloc, VecAlloc, VecAlloc);
    let mut out = alloc::vec![0u8; size];
    let (mut avail_in, mut in_off) = (input.len(), 0usize);
    let (mut avail_out, mut out_off, mut total) = (size, 0usize, 0usize);
    match BrotliDecompressStream(
        &mut avail_in,
        &mut in_off,
        input,
        &mut avail_out,
        &mut out_off,
        &mut out,
        &mut total,
        &mut state,
    ) {
        BrotliResult::ResultSuccess if out_off == size => Some(out),
        _ => None,
    }
}

const WOFF2_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

fn base128(d: &[u8], p: &mut usize) -> Option<u32> {
    let mut v: u32 = 0;
    for i in 0..5 {
        let b = *d.get(*p)?;
        *p += 1;
        if i == 0 && b == 0x80 {
            return None;
        }
        if v & 0xFE00_0000 != 0 {
            return None;
        }
        v = (v << 7) | u32::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// `255UInt16`.
fn read_255(d: &[u8], p: &mut usize) -> Option<u16> {
    let c = *d.get(*p)?;
    *p += 1;
    Some(match c {
        253 => {
            let v = rd_u16(d, *p)?;
            *p += 2;
            v
        }
        255 => 253 + u16::from(*d.get(*p).inspect(|_| *p += 1)?),
        254 => 506 + u16::from(*d.get(*p).inspect(|_| *p += 1)?),
        _ => u16::from(c),
    })
}

struct Stream<'a> {
    d: &'a [u8],
    p: usize,
}

impl Stream<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let v = rd_u16(self.d, self.p)?;
        self.p += 2;
        Some(v)
    }
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
}

/// The `glyf` transform undone (WOFF2 5.1): (glyf, loca, index format,
/// each glyph's xMin for an hmtx transform).
type Glyf = (Vec<u8>, Vec<u8>, u16, Vec<i16>);

fn reconstruct_glyf(t: &[u8]) -> Option<Glyf> {
    let option_flags = rd_u16(t, 2)?;
    let num_glyphs = rd_u16(t, 4)? as usize;
    let index_format = rd_u16(t, 6)?;
    let mut sizes = [0usize; 7];
    for (i, s) in sizes.iter_mut().enumerate() {
        *s = rd_u32(t, 8 + 4 * i)? as usize;
    }
    let mut off = 36;
    let mut streams = Vec::with_capacity(7);
    for s in sizes {
        streams.push(Stream {
            d: t.get(off..off + s)?,
            p: 0,
        });
        off += s;
    }
    let _ = option_flags;
    let mut it = streams.into_iter();
    let (mut ncont, mut npts, mut flags, mut glyph, mut comp, mut bbox, mut instr) = (
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
    );
    let bitmap_len = 4 * num_glyphs.div_ceil(32);
    let bitmap = bbox.take(bitmap_len)?.to_vec();
    let has_bbox = |i: usize| bitmap[i >> 3] & (0x80 >> (i & 7)) != 0;
    let mut glyf = Vec::new();
    let mut loca = Vec::with_capacity(num_glyphs + 1);
    let mut x_mins = Vec::with_capacity(num_glyphs);
    for i in 0..num_glyphs {
        loca.push(glyf.len());
        let n = ncont.u16()? as i16;
        if n == 0 {
            x_mins.push(0);
            continue;
        }
        let start = glyf.len();
        glyf.extend_from_slice(&n.to_be_bytes());
        glyf.extend_from_slice(&[0; 8]);
        if n < 0 {
            // composite: copy the components
            let mut have_instr = false;
            loop {
                let fl = comp.u16()?;
                let mut len = 2 + if fl & 1 != 0 { 4 } else { 2 };
                if fl & 8 != 0 {
                    len += 2;
                } else if fl & 0x40 != 0 {
                    len += 4;
                } else if fl & 0x80 != 0 {
                    len += 8;
                }
                glyf.extend_from_slice(&fl.to_be_bytes());
                glyf.extend_from_slice(comp.take(len)?);
                have_instr |= fl & 0x100 != 0;
                if fl & 0x20 == 0 {
                    break;
                }
            }
            if !has_bbox(i) {
                return None;
            }
            let b = bbox.take(8)?;
            glyf[start + 2..start + 10].copy_from_slice(b);
            if have_instr {
                let l = read_255(glyph.d, &mut glyph.p)? as usize;
                glyf.extend_from_slice(&(l as u16).to_be_bytes());
                glyf.extend_from_slice(instr.take(l)?);
            }
        } else {
            let mut ends = Vec::with_capacity(n as usize);
            let mut total = 0usize;
            for _ in 0..n {
                total += read_255(npts.d, &mut npts.p)? as usize;
                ends.push(total);
            }
            let (mut x, mut y) = (0i32, 0i32);
            let mut pts = Vec::with_capacity(total);
            for _ in 0..total {
                let f = flags.u8()?;
                let on = f & 0x80 == 0;
                let f = f & 0x7f;
                let sign = |fl: u8, v: i32| if fl & 1 != 0 { v } else { -v };
                let (dx, dy) = if f < 10 {
                    let b0 = i32::from(glyph.u8()?);
                    (0, sign(f, (i32::from(f & 0x0e) << 7) + b0))
                } else if f < 20 {
                    let b0 = i32::from(glyph.u8()?);
                    (sign(f, (i32::from((f - 10) & 0x0e) << 7) + b0), 0)
                } else if f < 84 {
                    let b0 = i32::from(f - 20);
                    let b1 = i32::from(glyph.u8()?);
                    (
                        sign(f, 1 + (b0 & 0x30) + (b1 >> 4)),
                        sign(f >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f)),
                    )
                } else if f < 120 {
                    let b0 = i32::from(f - 84);
                    let (i0, i1) = (i32::from(glyph.u8()?), i32::from(glyph.u8()?));
                    (
                        sign(f, 1 + ((b0 / 12) << 8) + i0),
                        sign(f >> 1, 1 + (((b0 % 12) >> 2) << 8) + i1),
                    )
                } else if f < 124 {
                    let (i0, i1, i2) = (
                        i32::from(glyph.u8()?),
                        i32::from(glyph.u8()?),
                        i32::from(glyph.u8()?),
                    );
                    (
                        sign(f, (i0 << 4) + (i1 >> 4)),
                        sign(f >> 1, ((i1 & 0x0f) << 8) + i2),
                    )
                } else {
                    let b = glyph.take(4)?;
                    (
                        sign(f, (i32::from(b[0]) << 8) + i32::from(b[1])),
                        sign(f >> 1, (i32::from(b[2]) << 8) + i32::from(b[3])),
                    )
                };
                x += dx;
                y += dy;
                pts.push((x, y, on));
            }
            let l = read_255(glyph.d, &mut glyph.p)? as usize;
            let ins = instr.take(l)?.to_vec();
            if has_bbox(i) {
                let b = bbox.take(8)?;
                glyf[start + 2..start + 10].copy_from_slice(b);
            } else {
                let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
                for &(px, py, _) in &pts {
                    x0 = x0.min(px);
                    y0 = y0.min(py);
                    x1 = x1.max(px);
                    y1 = y1.max(py);
                }
                if pts.is_empty() {
                    (x0, y0, x1, y1) = (0, 0, 0, 0);
                }
                for (k, v) in [x0, y0, x1, y1].into_iter().enumerate() {
                    glyf[start + 2 + 2 * k..start + 4 + 2 * k]
                        .copy_from_slice(&(v as i16).to_be_bytes());
                }
            }
            for e in &ends {
                glyf.extend_from_slice(&((*e - 1) as u16).to_be_bytes());
            }
            glyf.extend_from_slice(&(l as u16).to_be_bytes());
            glyf.extend_from_slice(&ins);
            for &(_, _, on) in &pts {
                glyf.push(u8::from(on));
            }
            let (mut px, mut py) = (0i32, 0i32);
            for &(x, _, _) in &pts {
                glyf.extend_from_slice(&((x - px) as i16).to_be_bytes());
                px = x;
            }
            for &(_, y, _) in &pts {
                glyf.extend_from_slice(&((y - py) as i16).to_be_bytes());
                py = y;
            }
        }
        x_mins.push(rd_u16(&glyf, start + 2).map_or(0, |v| v as i16));
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
    }
    loca.push(glyf.len());
    // Long offsets whatever the font had: the glyphs written here (flags
    // unpacked, coordinates as words) can outgrow short ones.
    let _ = index_format;
    let mut loca_bytes = Vec::new();
    for o in loca {
        loca_bytes.extend_from_slice(&(o as u32).to_be_bytes());
    }
    Some((glyf, loca_bytes, 1, x_mins))
}

/// The `hmtx` transform undone (WOFF2 5.4).
fn reconstruct_hmtx(t: &[u8], num_glyphs: usize, num_h: usize, x_mins: &[i16]) -> Option<Vec<u8>> {
    let flags = *t.first()?;
    let mut p = 1;
    let mut adv = Vec::with_capacity(num_h);
    for _ in 0..num_h {
        adv.push(rd_u16(t, p)?);
        p += 2;
    }
    let mut lsb = Vec::with_capacity(num_glyphs);
    for i in 0..num_h {
        if flags & 1 == 0 {
            lsb.push(rd_u16(t, p)?);
            p += 2;
        } else {
            lsb.push(*x_mins.get(i)? as u16);
        }
    }
    for i in num_h..num_glyphs {
        if flags & 2 == 0 {
            lsb.push(rd_u16(t, p)?);
            p += 2;
        } else {
            lsb.push(*x_mins.get(i)? as u16);
        }
    }
    let mut out = Vec::with_capacity(4 * num_h + 2 * (num_glyphs - num_h));
    for i in 0..num_glyphs {
        if i < num_h {
            out.extend_from_slice(&adv[i].to_be_bytes());
        }
        out.extend_from_slice(&lsb[i].to_be_bytes());
    }
    Some(out)
}

/// The sfnt a (single-font) WOFF2 file holds, as FreeType's
/// `woff2_open_font` rebuilds it (`glyf`/`loca` and `hmtx` transforms
/// undone), or `None`.
#[must_use]
pub fn decode2(data: &[u8]) -> Option<Vec<u8>> {
    if !is_woff2(data) {
        return None;
    }
    let flavor = rd_u32(data, 4)?;
    if flavor == u32::from_be_bytes(*b"ttcf") {
        return None;
    }
    let n = rd_u16(data, 12)? as usize;
    let comp_size = rd_u32(data, 20)? as usize;
    let mut p = 48;
    let mut dir = Vec::with_capacity(n);
    let mut total = 0usize;
    for _ in 0..n {
        let flags = *data.get(p)?;
        p += 1;
        let tag: [u8; 4] = if flags & 0x3f == 0x3f {
            let t = data.get(p..p + 4)?.try_into().ok()?;
            p += 4;
            t
        } else {
            *WOFF2_TAGS[(flags & 0x3f) as usize]
        };
        let version = (flags >> 6) & 3;
        let orig = base128(data, &mut p)? as usize;
        let glyf_loca = &tag == b"glyf" || &tag == b"loca";
        let transformed = if glyf_loca {
            version == 0
        } else {
            version != 0
        };
        let len = if transformed {
            base128(data, &mut p)? as usize
        } else {
            orig
        };
        dir.push((tag, len, transformed));
        total += len;
    }
    let stream = brotli(data.get(p..p + comp_size)?, total)?;
    let mut raw: Vec<([u8; 4], &[u8], bool)> = Vec::with_capacity(n);
    let mut q = 0;
    for (tag, len, tr) in &dir {
        raw.push((*tag, stream.get(q..q + len)?, *tr));
        q += len;
    }
    let find = |t: &[u8; 4]| raw.iter().find(|r| &r.0 == t);
    let num_glyphs = find(b"maxp").and_then(|r| rd_u16(r.1, 4)).unwrap_or(0) as usize;
    let num_h = find(b"hhea").and_then(|r| rd_u16(r.1, 34)).unwrap_or(0) as usize;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(n);
    let mut x_mins = Vec::new();
    let mut head_index_format = None;
    for (tag, b, tr) in &raw {
        match (tag, tr) {
            (b"glyf", true) => {
                let (g, l, fmt, xm) = reconstruct_glyf(b)?;
                x_mins = xm;
                head_index_format = Some(fmt);
                tables.push((*b"glyf", g));
                tables.push((*b"loca", l));
            }
            (b"loca" | b"hmtx", true) => {}
            _ => tables.push((*tag, b.to_vec())),
        }
    }
    if let Some((_, b, _)) = raw.iter().find(|r| &r.0 == b"hmtx" && r.2) {
        tables.push((*b"hmtx", reconstruct_hmtx(b, num_glyphs, num_h, &x_mins)?));
    }
    if let (Some(fmt), Some(head)) = (
        head_index_format,
        tables.iter_mut().find(|t| &t.0 == b"head"),
    ) && head.1.len() >= 52
    {
        head.1[50..52].copy_from_slice(&fmt.to_be_bytes());
    }
    tables.sort_by_key(|t| t.0);
    Some(build_sfnt(flavor, &tables))
}

fn build_sfnt(flavor: u32, tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len();
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    let mut sr = 1u16;
    let mut es = 0u16;
    while usize::from(sr) * 2 <= n {
        sr *= 2;
        es += 1;
    }
    out.extend_from_slice(&(sr * 16).to_be_bytes());
    out.extend_from_slice(&es.to_be_bytes());
    out.extend_from_slice(&((n as u16) * 16 - sr * 16).to_be_bytes());
    let mut pos = 12 + 16 * n;
    for (tag, b) in tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(pos as u32).to_be_bytes());
        out.extend_from_slice(&(b.len() as u32).to_be_bytes());
        pos += (b.len() + 3) & !3;
    }
    for (_, b) in tables {
        out.extend_from_slice(b);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

/// The sfnt of a WOFF or WOFF2 file (`None` for anything else).
#[must_use]
pub fn unpack(data: &[u8]) -> Option<Vec<u8>> {
    if is_woff(data) {
        decode(data)
    } else {
        decode2(data)
    }
}
