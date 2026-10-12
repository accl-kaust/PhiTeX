//! `writettf.c`: TrueType font files (`/FontFile2`), whole or subset to
//! the glyphs used.

use std::collections::BTreeSet;

use crate::PdfError;
use crate::enc::GlyphNames;
use crate::macnames::{AMBIGUOUS_NAMES, MAC_GLYPH_NAMES};
use crate::map::MapEntry;
use crate::t1::{FontDesc, dim, make_subset_tag};

const DEFAULT_NTABS: u16 = 14;
const NEW_CMAP_SIZE: u16 = 2;
const NMACGLYPHS: usize = 258;
const TABDIR_OFF: usize = 12;

const ARG_1_AND_2_ARE_WORDS: u16 = 1 << 0;
const WE_HAVE_A_SCALE: u16 = 1 << 3;
const MORE_COMPONENTS: u16 = 1 << 5;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 1 << 6;
const WE_HAVE_A_TWO_BY_TWO: u16 = 1 << 7;
const WE_HAVE_INSTRUCTIONS: u16 = 1 << 8;

/// `newtabnames`.
const NEW_TAB_NAMES: [&[u8; 4]; 14] = [
    b"OS/2", b"PCLT", b"cmap", b"cvt ", b"fpgm", b"glyf", b"head", b"hhea", b"hmtx", b"loca",
    b"maxp", b"name", b"post", b"prep",
];

/// `BYTE_ENCODING_LENGTH`.
const BYTE_ENCODING_LENGTH: u32 = 256 + 3 * 2;
/// `TRIMMED_TABLE_MAP_LENGTH`.
const TRIMMED_TABLE_MAP_LENGTH: u32 = 2 * (5 + 256);
/// `SEG_MAP_DELTA_LENGTH`.
const SEG_MAP_DELTA_LENGTH: u32 = (16 + 256) * 2;
/// `CMAP_ENTRY_LENGTH`.
const CMAP_ENTRY_LENGTH: u32 = 2 * 2 + 4;

/// A glyph's name: `None` is the `notdef` of `macnames.c`.
type Name = Option<Vec<u8>>;

/// `glyph_entry`.
#[derive(Clone, Default)]
struct Glyph {
    offset: u32,
    newoffset: u32,
    adv_width: u16,
    lsb: u16,
    name: Name,
    newindex: i32,
    name_index: u16,
}

/// `dirtab_entry`.
#[derive(Clone, Copy)]
struct DirTab {
    tag: [u8; 4],
    checksum: u32,
    offset: u32,
    length: u32,
}

/// `name_record`.
#[derive(Clone, Copy, Default)]
struct NameRecord {
    platform_id: u16,
    encoding_id: u16,
    language_id: u16,
    name_id: u16,
    length: u16,
    offset: u16,
    new_offset: u16,
    new_length: u16,
}

/// `ttfenc_entry` (no subfonts).
#[derive(Clone, Default)]
struct EncEntry {
    name: Name,
    newindex: i32,
}

fn fail(m: impl Into<String>) -> PdfError {
    PdfError::Font(format!("writettf: {}", m.into()))
}

/// `ttf_funit`.
fn funit(n: i64, upem: i64) -> i32 {
    let v = if n < 0 {
        -((-n / upem) * 1000 + ((-n % upem) * 1000) / upem)
    } else {
        (n / upem) * 1000 + ((n % upem) * 1000) / upem
    };
    i32::try_from(v).unwrap_or(0)
}

/// `strip_spaces_and_delims`.
fn strip_spaces_and_delims(s: &[u8]) -> Vec<u8> {
    s.iter()
        .take_while(|&&c| c != 0)
        .copied()
        .filter(|c| !b"()<>[]{}/%".contains(c) && !c.is_ascii_whitespace() && *c != 0x0B)
        .collect()
}

/// C's `sscanf(s, "<prefix>%X%n")` (or `%i`) consuming all of `s`.
fn scan_number(s: &[u8], prefix: &[u8], hex: bool) -> Option<i64> {
    let r = s.strip_prefix(prefix)?;
    let r = std::str::from_utf8(r).ok()?;
    let (neg, r) = match r.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, r.strip_prefix('+').unwrap_or(r)),
    };
    let digits = |r: &str, radix: u32| -> Option<i64> {
        if r.is_empty() || !r.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        i64::from_str_radix(r, radix).ok()
    };
    let hex_part = r.strip_prefix("0x").or_else(|| r.strip_prefix("0X"));
    let v = if hex {
        digits(hex_part.unwrap_or(r), 16)?
    } else if let Some(h) = hex_part {
        digits(h, 16)?
    } else if r.len() > 1 && r.starts_with('0') {
        digits(r, 8)?
    } else {
        digits(r, 10)?
    };
    Some(if neg { -v } else { v })
}

fn mac_name(i: usize) -> Name {
    if i == 0 {
        None
    } else {
        MAC_GLYPH_NAMES.get(i).map(|s| s.as_bytes().to_vec())
    }
}

struct Ttf<'a> {
    data: &'a [u8],
    pos: usize,
    out: Vec<u8>,
    opos: usize,
    checksum: u32,
    tab_length: u32,
    tmp_ulong: u32,
    ntabs: u16,
    upem: u16,
    post_format: u32,
    loca_format: i16,
    last_glyf_offset: u32,
    glyphs_count: u16,
    new_glyphs_count: u16,
    nhmtxs: u16,
    new_ntabs: u16,
    glyphs: Vec<Glyph>,
    glyph_index: Vec<usize>,
    names: Vec<NameRecord>,
    name_buf: Vec<u8>,
    dir: Vec<DirTab>,
    check_sum_adjustment_offset: usize,
    enc: Vec<EncEntry>,
    /// The `(3,1)` or `(0,3)` cmap.
    cmap: Option<Vec<i64>>,
    /// Whether `cmap` was looked for.
    cmap_read: bool,
}

impl Ttf<'_> {
    // ---- input ----

    fn getnum(&mut self, s: usize) -> Result<u32, PdfError> {
        let mut i: u32 = 0;
        for _ in 0..s {
            let c = *self
                .data
                .get(self.pos)
                .ok_or_else(|| fail("unexpected EOF"))?;
            self.pos += 1;
            i = (i << 8).wrapping_add(u32::from(c));
        }
        Ok(i)
    }

    fn skip(&mut self, n: usize) -> Result<(), PdfError> {
        self.getnum(n).map(|_| ())
    }

    fn get_u8(&mut self) -> Result<u8, PdfError> {
        Ok(u8::try_from(self.getnum(1)?).unwrap_or(0))
    }

    fn get_u16(&mut self) -> Result<u16, PdfError> {
        Ok(u16::try_from(self.getnum(2)?).unwrap_or(0))
    }

    fn get_i16(&mut self) -> Result<i16, PdfError> {
        Ok(self.get_u16()?.cast_signed())
    }

    fn get_u32(&mut self) -> Result<u32, PdfError> {
        self.getnum(4)
    }

    // ---- output (`fb_*`) ----

    fn putchar(&mut self, b: u8) {
        if self.opos >= self.out.len() {
            self.out.resize(self.opos + 1, 0);
        }
        self.out[self.opos] = b;
        self.opos += 1;
    }

    fn seek_out(&mut self, off: usize) {
        if off > self.out.len() {
            self.out.resize(off, 0);
        }
        self.opos = off;
    }

    /// `ttf_addchksm`.
    fn addchksm(&mut self, b: u8) -> u8 {
        self.tmp_ulong = (self.tmp_ulong << 8).wrapping_add(u32::from(b));
        self.tab_length = self.tab_length.wrapping_add(1);
        if self.tab_length.is_multiple_of(4) {
            self.checksum = self.checksum.wrapping_add(self.tmp_ulong);
            self.tmp_ulong = 0;
        }
        b
    }

    /// `ttf_getchksm`.
    fn getchksm(&mut self) -> u32 {
        while !self.tab_length.is_multiple_of(4) {
            let b = self.addchksm(0);
            self.putchar(b);
        }
        self.checksum
    }

    /// `ttf_putnum`.
    fn putnum(&mut self, s: usize, n: i64) {
        for k in (0..s).rev() {
            let b = u8::try_from((n >> (8 * k)) & 0xFF).unwrap_or(0);
            let b = self.addchksm(b);
            self.putchar(b);
        }
    }

    fn put_u8(&mut self, n: i64) {
        self.putnum(1, n);
    }

    fn put_u16(&mut self, n: i64) {
        self.putnum(2, n);
    }

    fn put_u32(&mut self, n: i64) {
        self.putnum(4, n);
    }

    fn ncopy(&mut self, n: usize) -> Result<(), PdfError> {
        for _ in 0..n {
            let b = self.get_u8()?;
            self.put_u8(i64::from(b));
        }
        Ok(())
    }

    fn copy_u16(&mut self) -> Result<u16, PdfError> {
        let v = self.get_u16()?;
        self.put_u16(i64::from(v));
        Ok(v)
    }

    // ---- tables ----

    /// `ttf_name_lookup`.
    fn lookup(&self, s: [u8; 4]) -> Option<usize> {
        self.dir.iter().position(|t| t.tag == s)
    }

    fn lookup_req(&self, s: [u8; 4]) -> Result<usize, PdfError> {
        self.lookup(s).ok_or_else(|| {
            fail(format!(
                "can't find table `{}'",
                String::from_utf8_lossy(&s)
            ))
        })
    }

    /// `ttf_seek_tab`.
    fn seek_tab(&mut self, s: [u8; 4], offset: usize) -> Result<usize, PdfError> {
        let t = self.lookup_req(s)?;
        self.pos = self.dir[t].offset as usize + offset;
        Ok(t)
    }

    fn read_tabdir(&mut self) -> Result<(), PdfError> {
        self.skip(4)?;
        self.ntabs = self.get_u16()?;
        self.skip(6)?;
        for _ in 0..self.ntabs {
            let mut tag = [0u8; 4];
            for t in &mut tag {
                *t = self.get_u8()?;
            }
            let checksum = self.get_u32()?;
            let offset = self.get_u32()?;
            let length = self.get_u32()?;
            self.dir.push(DirTab {
                tag,
                checksum,
                offset,
                length,
            });
        }
        Ok(())
    }

    fn read_maxp(&mut self) -> Result<(), PdfError> {
        self.seek_tab(*b"maxp", 4)?;
        self.glyphs_count = self.get_u16()?;
        let n = usize::from(self.glyphs_count) + 1;
        self.glyphs = vec![
            Glyph {
                newindex: -1,
                ..Glyph::default()
            };
            n
        ];
        self.glyph_index = vec![0; n.max(2)];
        self.glyph_index[1] = 1;
        Ok(())
    }

    fn read_head(&mut self, fd: &mut FontDesc) -> Result<(), PdfError> {
        self.seek_tab(*b"head", 2 * 4 + 2 * 4 + 2)?;
        self.upem = self.get_u16()?;
        if self.upem == 0 {
            return Err(fail("unitsPerEm is 0"));
        }
        self.skip(16)?;
        let upem = i64::from(self.upem);
        for k in 0..4 {
            let v = self.get_i16()?;
            fd.font_dim[dim::FONTBBOX1 + k] = (funit(i64::from(v), upem), true);
        }
        self.skip(2 * 2 + 2)?;
        self.loca_format = self.get_i16()?;
        Ok(())
    }

    fn read_hhea(&mut self, fd: &mut FontDesc) -> Result<(), PdfError> {
        self.seek_tab(*b"hhea", 4)?;
        let upem = i64::from(self.upem);
        let a = self.get_i16()?;
        fd.font_dim[dim::ASCENT] = (funit(i64::from(a), upem), true);
        let d = self.get_i16()?;
        fd.font_dim[dim::DESCENT] = (funit(i64::from(d), upem), true);
        self.skip(2 + 2 + 3 * 2 + 8 * 2)?;
        self.nhmtxs = self.get_u16()?;
        Ok(())
    }

    fn read_pclt(&mut self, fd: &mut FontDesc) -> Result<(), PdfError> {
        if self.lookup(*b"PCLT").is_none() {
            return Ok(());
        }
        self.seek_tab(*b"PCLT", 4 + 4 + 2)?;
        let upem = i64::from(self.upem);
        let x = self.get_u16()?;
        fd.font_dim[dim::XHEIGHT].0 = funit(i64::from(x), upem);
        self.skip(2 * 2)?;
        let c = self.get_u16()?;
        fd.font_dim[dim::CAPHEIGHT].0 = funit(i64::from(c), upem);
        fd.font_dim[dim::XHEIGHT].1 = true;
        fd.font_dim[dim::CAPHEIGHT].1 = true;
        Ok(())
    }

    fn read_hmtx(&mut self) -> Result<(), PdfError> {
        self.seek_tab(*b"hmtx", 0)?;
        let n = usize::from(self.glyphs_count);
        let m = usize::from(self.nhmtxs).min(n);
        for g in 0..m {
            self.glyphs[g].adv_width = self.get_u16()?;
            self.glyphs[g].lsb = self.get_u16()?;
        }
        if m < n {
            let last = if m > 0 {
                self.glyphs[m - 1].adv_width
            } else {
                0
            };
            for g in m..n {
                self.glyphs[g].adv_width = last;
                self.glyphs[g].lsb = self.get_u16()?;
            }
        }
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation)]
    fn read_post(&mut self, fd: &mut FontDesc) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"post", 0)?;
        self.post_format = self.get_u32()?;
        let italic_angle = self.get_u32()?;
        let mut int_part = i64::from(italic_angle >> 16);
        let mut sign = 1.0;
        if int_part > 0x7FFF {
            int_part = 0x10000 - int_part;
            sign = -1.0;
        }
        let frac_part = i64::from(italic_angle % 0x10000);
        #[allow(clippy::cast_precision_loss)]
        let a = sign * (int_part as f64 + frac_part as f64 / f64::from(0x10000));
        fd.font_dim[dim::ITALIC_ANGLE] = (a as i32, true);
        self.skip(2 * 2 + 5 * 4)?;
        let n = self.glyphs.len();
        match self.post_format {
            0x10000 => {
                for g in 0..NMACGLYPHS.min(n) {
                    self.glyphs[g].name = mac_name(g);
                    self.glyphs[g].name_index = g as u16;
                }
            }
            0x20000 => {
                let nnames = usize::from(self.get_u16()?);
                for g in 0..nnames {
                    let v = self.get_u16()?;
                    if let Some(x) = self.glyphs.get_mut(g) {
                        x.name_index = v;
                    }
                }
                let tab = self.dir[t];
                let used = self.pos - tab.offset as usize;
                let length = (tab.length as usize).saturating_sub(used);
                let mut strings: Vec<Vec<u8>> = Vec::new();
                let mut p = 0;
                while p < length {
                    let k = usize::from(self.get_u8()?);
                    let mut s = Vec::with_capacity(k);
                    for _ in 0..k {
                        s.push(self.get_u8()?);
                    }
                    p += k + 1;
                    strings.push(s);
                }
                for g in 0..nnames.min(n) {
                    let ni = usize::from(self.glyphs[g].name_index);
                    self.glyphs[g].name = if ni < NMACGLYPHS {
                        mac_name(ni)
                    } else {
                        Some(strings.get(ni - NMACGLYPHS).cloned().unwrap_or_default())
                    };
                }
            }
            _ => {
                // format 3.0, and the others taken as 3.0
                for g in 0..NMACGLYPHS.min(n) {
                    self.glyphs[g].name_index = g as u16;
                }
            }
        }
        Ok(())
    }

    fn read_loca(&mut self) -> Result<(), PdfError> {
        self.seek_tab(*b"loca", 0)?;
        for g in 0..self.glyphs.len() {
            self.glyphs[g].offset = if self.loca_format != 0 {
                self.get_u32()?
            } else {
                u32::from(self.get_u16()?) << 1
            };
        }
        Ok(())
    }

    fn read_name(&mut self, fd: &mut FontDesc) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"name", 2)?;
        let num = self.get_u16()?;
        let size = i64::from(self.dir[t].length) - (3 * 2 + i64::from(num) * 6 * 2);
        let size = usize::try_from(size).map_err(|_| fail("invalid name table"))?;
        self.skip(2)?;
        for _ in 0..num {
            let r = NameRecord {
                platform_id: self.get_u16()?,
                encoding_id: self.get_u16()?,
                language_id: self.get_u16()?,
                name_id: self.get_u16()?,
                length: self.get_u16()?,
                offset: self.get_u16()?,
                ..NameRecord::default()
            };
            self.names.push(r);
        }
        for _ in 0..size {
            let b = self.get_u8()?;
            self.name_buf.push(b);
        }
        let slice = |n: &NameRecord, buf: &[u8]| -> Vec<u8> {
            let a = usize::from(n.offset).min(buf.len());
            let b = (a + usize::from(n.length)).min(buf.len());
            buf[a..b].to_vec()
        };
        // look for the PostScript name
        if let Some(n) = self
            .names
            .iter()
            .find(|n| n.platform_id == 1 && n.encoding_id == 0 && n.name_id == 6)
        {
            fd.fontname = strip_spaces_and_delims(&slice(n, &self.name_buf));
            fd.font_dim[dim::FONTNAME].1 = true;
        }
        if !fd.font_dim[dim::FONTNAME].1
            && let Some(n) = self.names.iter().find(|n| {
                n.platform_id == 3 && (n.encoding_id == 0 || n.encoding_id == 1) && n.name_id == 6
            })
        {
            let s = slice(n, &self.name_buf);
            let buf: Vec<u8> = s
                .chunks(2)
                .filter_map(|c| c.get(1).copied())
                .take_while(|&c| c != 0)
                .collect();
            fd.fontname = strip_spaces_and_delims(&buf);
            fd.font_dim[dim::FONTNAME].1 = true;
        }
        Ok(())
    }

    fn read_font(&mut self, fd: &mut FontDesc) -> Result<(), PdfError> {
        self.read_tabdir()?;
        for t in [b"PCLT", b"fpgm", b"cvt ", b"prep"] {
            if self.lookup(*t).is_none() {
                self.new_ntabs -= 1;
            }
        }
        self.read_maxp()?;
        self.read_head(fd)?;
        self.read_hhea(fd)?;
        self.read_pclt(fd)?;
        self.read_hmtx()?;
        self.read_post(fd)?;
        self.read_loca()?;
        self.read_name(fd)
    }

    // ---- writing ----

    /// `ttf_reset_chksm`.
    fn reset_chksm(&mut self, t: usize) {
        self.checksum = 0;
        self.tab_length = 0;
        self.tmp_ulong = 0;
        self.dir[t].offset = u32::try_from(self.opos).unwrap_or(0);
    }

    /// `ttf_set_chksm`.
    fn set_chksm(&mut self, t: usize) {
        self.dir[t].length = u32::try_from(self.opos).unwrap_or(0) - self.dir[t].offset;
        self.dir[t].checksum = self.getchksm();
    }

    fn copytab(&mut self, s: [u8; 4]) -> Result<(), PdfError> {
        let t = self.seek_tab(s, 0)?;
        self.reset_chksm(t);
        self.ncopy(self.dir[t].length as usize)?;
        self.set_chksm(t);
        Ok(())
    }

    /// `ttf_copy_encoding` (a reencoded font).
    fn copy_encoding(
        &mut self,
        names: &GlyphNames,
        fd: &mut FontDesc,
        tags: &mut BTreeSet<[u8; 6]>,
    ) {
        self.enc = vec![EncEntry::default(); 256];
        let tx = fd.tx_tree.get_or_insert_with(BTreeSet::new);
        // a workaround for a bug of AcroReader 4.0
        if names.get(97).and_then(|n| n.as_deref()) == Some(b"a") {
            tx.insert(97);
        }
        for &q in tx.iter() {
            if q < 256 {
                self.enc[q].name = names.get(q).cloned().flatten();
            }
        }
        fd.subset_tag = Some(make_subset_tag(&fd.gl_tree, &fd.fontname, tags));
    }

    /// `ttf_read_cmap` for `(pid, eid)`: format 4 only.
    fn read_cmap(&mut self, pid: u16, eid: u16) -> Result<Option<Vec<i64>>, PdfError> {
        self.seek_tab(*b"cmap", 2)?;
        let n = self.get_u16()?;
        let cmap_offset = self.pos - 2 * 2;
        let mut found = None;
        for _ in 0..n {
            let p = self.get_u16()?;
            let e = self.get_u16()?;
            let off = self.get_u32()?;
            if p == pid && e == eid {
                found = Some(off);
                break;
            }
        }
        let Some(off) = found else {
            return Ok(None);
        };
        self.pos = cmap_offset + off as usize;
        if self.get_u16()? != 4 {
            return Ok(None);
        }
        let mut table = vec![-1i64; 0x10000];
        let mut length = i64::from(self.get_u16()?);
        self.get_u16()?;
        let seg_count = usize::from(self.get_u16()? / 2);
        self.skip(6)?;
        let read = |t: &mut Self| -> Result<Vec<u16>, PdfError> {
            (0..seg_count).map(|_| t.get_u16()).collect()
        };
        let end = read(self)?;
        self.get_u16()?;
        let start = read(self)?;
        let delta = read(self)?;
        let range = read(self)?;
        let segs = i64::try_from(seg_count).unwrap_or(0);
        length -= 8 * 2 + 4 * segs * 2;
        let n = usize::try_from(length / 2).unwrap_or(0);
        let glyph_id: Vec<u16> = (0..n).map(|_| self.get_u16()).collect::<Result<_, _>>()?;
        for s in 0..seg_count {
            for i in i64::from(start[s])..=i64::from(end[s]) {
                if i == 0xFFFF {
                    break;
                }
                if range[s] == 0xFFFF {
                    continue;
                }
                let index = if range[s] == 0 {
                    (i64::from(delta[s]) + i) & 0xFFFF
                } else {
                    let k = (i - i64::from(start[s]))
                        + i64::from(range[s] / 2)
                        + i64::try_from(s).unwrap_or(0)
                        - segs;
                    let index = usize::try_from(k)
                        .ok()
                        .and_then(|k| glyph_id.get(k))
                        .map_or(0, |&g| i64::from(g));
                    if index == 0 {
                        0
                    } else {
                        (index + i64::from(delta[s])) & 0xFFFF
                    }
                };
                if index >= i64::from(self.glyphs_count) {
                    return Err(fail(format!(
                        "cmap: glyph index {index} out of range [0..{})",
                        self.glyphs_count
                    )));
                }
                let slot = &mut table[usize::try_from(i).unwrap_or(0)];
                if *slot == -1 {
                    *slot = index;
                }
            }
        }
        Ok(Some(table))
    }

    /// `ttf_reindex_glyphs` (reencoded fonts).
    fn reindex_glyphs(&mut self) -> Result<(), PdfError> {
        for e in 0..256 {
            self.enc[e].newindex = 0;
            let Some(name) = self.enc[e].name.clone() else {
                continue;
            };
            let mut found = self
                .glyphs
                .iter()
                .take(usize::from(self.glyphs_count))
                .position(|g| g.name.as_deref() == Some(&name[..]));
            if found.is_none()
                && let Some(index) = scan_number(&name, b"uni", true)
            {
                if !self.cmap_read {
                    let mut c = self.read_cmap(3, 1)?;
                    if c.is_none() {
                        c = self.read_cmap(0, 3)?;
                    }
                    self.cmap = c;
                    self.cmap_read = true;
                }
                let Some(t) = &self.cmap else {
                    continue;
                };
                let v = usize::try_from(index)
                    .ok()
                    .and_then(|i| t.get(i))
                    .copied()
                    .unwrap_or(-1);
                if v == -1 || v >= i64::from(self.glyphs_count) {
                    continue;
                }
                found = usize::try_from(v).ok();
            }
            if found.is_none()
                && let Some(index) = scan_number(&name, b"index", false)
            {
                if index < 0 || index >= i64::from(self.glyphs_count) {
                    continue;
                }
                found = usize::try_from(index).ok();
            }
            let Some(g) = found else {
                continue;
            };
            if self.glyphs[g].newindex < 0 {
                let n = usize::from(self.new_glyphs_count);
                self.glyph_index[n] = g;
                self.glyphs[g].newindex = i32::from(self.new_glyphs_count);
                self.new_glyphs_count += 1;
            }
            self.enc[e].newindex = self.glyphs[g].newindex;
        }
        Ok(())
    }

    fn write_glyf(&mut self) -> Result<(), PdfError> {
        let t = self.lookup_req(*b"glyf")?;
        let glyf_offset = self.dir[t].offset as usize;
        let new_glyf_offset = self.opos;
        self.reset_chksm(t);
        let mut i = 0;
        while i < usize::from(self.new_glyphs_count) {
            let id = self.glyph_index[i];
            self.glyphs[id].newoffset = u32::try_from(self.opos - new_glyf_offset).unwrap_or(0);
            let (o, o1) = (self.glyphs[id].offset, self.glyphs[id + 1].offset);
            if o != o1 {
                self.pos = glyf_offset + o as usize;
                let k = self.get_i16()?;
                self.put_u16(i64::from(k));
                self.ncopy(4 * 2)?;
                if k < 0 {
                    let mut flags;
                    loop {
                        flags = self.copy_u16()?;
                        let idx = usize::from(self.get_u16()?);
                        if idx >= self.glyphs.len() {
                            return Err(fail("invalid glyph index in a composite glyph"));
                        }
                        if self.glyphs[idx].newindex < 0 {
                            self.glyphs[idx].newindex = i32::from(self.new_glyphs_count);
                            let n = usize::from(self.new_glyphs_count);
                            self.glyph_index[n] = idx;
                            self.new_glyphs_count += 1;
                        }
                        self.put_u16(i64::from(self.glyphs[idx].newindex));
                        if flags & ARG_1_AND_2_ARE_WORDS != 0 {
                            self.ncopy(2 * 2)?;
                        } else {
                            self.ncopy(2)?;
                        }
                        if flags & WE_HAVE_A_SCALE != 0 {
                            self.ncopy(2)?;
                        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                            self.ncopy(2 * 2)?;
                        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
                            self.ncopy(4 * 2)?;
                        }
                        if flags & MORE_COMPONENTS == 0 {
                            break;
                        }
                    }
                    if flags & WE_HAVE_INSTRUCTIONS != 0 {
                        let n = self.copy_u16()?;
                        self.ncopy(usize::from(n))?;
                    }
                } else {
                    let n = i64::from(o1) - i64::from(o) - 2 - 4 * 2;
                    self.ncopy(usize::try_from(n).unwrap_or(0))?;
                }
            }
            i += 1;
        }
        self.last_glyf_offset = u32::try_from(self.opos - new_glyf_offset).unwrap_or(0);
        self.set_chksm(t);
        Ok(())
    }

    fn ids(&self) -> Vec<usize> {
        self.glyph_index[..usize::from(self.new_glyphs_count)].to_vec()
    }

    fn write_loca(&mut self) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"loca", 0)?;
        self.reset_chksm(t);
        let ids = self.ids();
        self.loca_format = i16::from(
            self.last_glyf_offset >= 0x0002_0000
                || self.last_glyf_offset & 1 != 0
                || ids.iter().any(|&id| self.glyphs[id].newoffset & 1 != 0),
        );
        if self.loca_format != 0 {
            for &id in &ids {
                self.put_u32(i64::from(self.glyphs[id].newoffset));
            }
            self.put_u32(i64::from(self.last_glyf_offset));
        } else {
            for &id in &ids {
                self.put_u16(i64::from(self.glyphs[id].newoffset / 2));
            }
            self.put_u16(i64::from(self.last_glyf_offset / 2));
        }
        self.set_chksm(t);
        Ok(())
    }

    fn write_os2(&mut self) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"OS/2", 0)?;
        self.reset_chksm(t);
        self.get_u16()?;
        self.put_u16(0x0001);
        self.ncopy(2 * 2 + 13 * 2 + 10)?;
        self.skip(4 * 4)?;
        self.put_u32(0x0000_0003);
        self.put_u32(0x1000_0000);
        self.put_u32(0);
        self.put_u32(0);
        self.ncopy(4 + 2)?;
        self.skip(2 * 2)?;
        self.put_u16(0x0000);
        self.put_u16(0xF0FF);
        self.ncopy(5 * 2)?;
        self.put_u32(0x8000_0000);
        self.put_u32(0);
        self.set_chksm(t);
        Ok(())
    }

    fn write_head(&mut self, subsetted: bool) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"head", 0)?;
        self.reset_chksm(t);
        self.ncopy(2 * 4)?;
        self.check_sum_adjustment_offset = self.opos;
        self.put_u32(0);
        self.skip(4)?;
        self.ncopy(4 + 2 * 2 + 16 + 4 * 2 + 2 * 2 + 2)?;
        if subsetted {
            self.put_u16(i64::from(self.loca_format));
            self.put_u16(0);
        } else {
            self.ncopy(2 * 2)?;
        }
        self.set_chksm(t);
        Ok(())
    }

    fn write_hhea(&mut self) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"hhea", 0)?;
        self.reset_chksm(t);
        self.ncopy(4 + 3 * 2 + 2 + 3 * 2 + 8 * 2)?;
        self.put_u16(i64::from(self.new_glyphs_count));
        self.set_chksm(t);
        Ok(())
    }

    fn write_hmtx(&mut self) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"hmtx", 0)?;
        self.reset_chksm(t);
        for id in self.ids() {
            let (a, l) = (self.glyphs[id].adv_width, self.glyphs[id].lsb);
            self.put_u16(i64::from(a));
            self.put_u16(i64::from(l));
        }
        self.set_chksm(t);
        Ok(())
    }

    fn write_maxp(&mut self) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"maxp", 4 + 2)?;
        self.reset_chksm(t);
        self.put_u32(0x0001_0000);
        self.put_u16(i64::from(self.new_glyphs_count));
        self.ncopy(13 * 2)?;
        self.set_chksm(t);
        Ok(())
    }

    fn write_name(&mut self, fd: &FontDesc) -> Result<(), PdfError> {
        let t = self.lookup_req(*b"name")?;
        let tag = fd.subset_tag.unwrap_or(*b"AAAAAA");
        let mut buf = Vec::new();
        for n in &mut self.names {
            n.new_offset = u16::try_from(buf.len()).unwrap_or(0);
            let l = if matches!(n.name_id, 1 | 3 | 4 | 6)
                && ((n.platform_id == 1 && n.encoding_id == 0)
                    || (n.platform_id == 3 && (n.encoding_id == 0 || n.encoding_id == 1)))
            {
                // prepend_subset_tags
                if n.platform_id == 3 {
                    for c in tag {
                        buf.push(0);
                        buf.push(c);
                    }
                    buf.push(0);
                    buf.push(b'+');
                    14
                } else {
                    buf.extend_from_slice(&tag);
                    buf.push(b'+');
                    7
                }
            } else {
                0
            };
            let a = usize::from(n.offset);
            for k in 0..usize::from(n.length) {
                buf.push(self.name_buf.get(a + k).copied().unwrap_or(0));
            }
            n.new_length = n.length + l;
        }
        self.reset_chksm(t);
        self.put_u16(0);
        let num = self.names.len();
        self.put_u16(i64::try_from(num).unwrap_or(0));
        self.put_u16(i64::try_from(3 * 2 + num * 6 * 2).unwrap_or(0));
        for n in self.names.clone() {
            for v in [
                n.platform_id,
                n.encoding_id,
                n.language_id,
                n.name_id,
                n.new_length,
                n.new_offset,
            ] {
                self.put_u16(i64::from(v));
            }
        }
        for b in buf {
            self.put_u8(i64::from(b));
        }
        self.set_chksm(t);
        Ok(())
    }

    fn write_post(&mut self, glyph_names: bool) -> Result<(), PdfError> {
        let t = self.seek_tab(*b"post", 4)?;
        self.reset_chksm(t);
        if !glyph_names || self.post_format == 0x0003_0000 {
            self.put_u32(0x0003_0000);
            self.ncopy(4 + 2 * 2 + 5 * 4)?;
        } else {
            self.put_u32(0x0002_0000);
            self.ncopy(4 + 2 * 2 + 5 * 4)?;
            self.put_u16(i64::from(self.new_glyphs_count));
            let ids = self.ids();
            let mut k = 0u16;
            for &id in &ids {
                let g = &mut self.glyphs[id];
                let unsafe_name = g
                    .name
                    .as_deref()
                    .is_some_and(|n| AMBIGUOUS_NAMES.iter().any(|a| a.as_bytes() == n));
                if usize::from(g.name_index) >= NMACGLYPHS || unsafe_name {
                    g.name_index = 258 + k;
                    k += 1;
                }
                let ni = g.name_index;
                self.put_u16(i64::from(ni));
            }
            for &id in &ids {
                if usize::from(self.glyphs[id].name_index) >= NMACGLYPHS {
                    let s = self.glyphs[id]
                        .name
                        .clone()
                        .unwrap_or_else(|| b".notdef".to_vec());
                    self.put_u8(i64::try_from(s.len()).unwrap_or(0));
                    for c in s {
                        self.put_u8(i64::from(c));
                    }
                }
            }
        }
        self.set_chksm(t);
        Ok(())
    }

    fn write_cmap(&mut self) -> Result<(), PdfError> {
        let t = self.lookup_req(*b"cmap")?;
        // ttf_select_cmap
        let fmt0 = if self.new_glyphs_count < 256 { 0 } else { 6 };
        let tabs = [(1u16, 0u16, fmt0), (3, 0, 4)];
        self.reset_chksm(t);
        self.put_u16(0);
        self.put_u16(i64::from(NEW_CMAP_SIZE));
        let mut offset = 2 * 2 + u32::from(NEW_CMAP_SIZE) * CMAP_ENTRY_LENGTH;
        for &(p, e, f) in &tabs {
            let this = offset;
            offset += match f {
                0 => BYTE_ENCODING_LENGTH,
                4 => SEG_MAP_DELTA_LENGTH,
                _ => TRIMMED_TABLE_MAP_LENGTH,
            };
            self.put_u16(i64::from(p));
            self.put_u16(i64::from(e));
            self.put_u32(i64::from(this));
        }
        let idx: Vec<i32> = self.enc.iter().map(|e| e.newindex).collect();
        for &(_, _, f) in &tabs {
            let head: &[i64] = match f {
                0 => &[0, i64::from(BYTE_ENCODING_LENGTH), 0],
                4 => &[
                    4,
                    i64::from(SEG_MAP_DELTA_LENGTH),
                    0,
                    4,
                    4,
                    1,
                    0,
                    0xF0FF,
                    0xFFFF,
                    0,
                    0xF000,
                    0xFFFF,
                    0,
                    1,
                    2 * 2,
                    0,
                ],
                _ => &[6, i64::from(TRIMMED_TABLE_MAP_LENGTH), 0, 0, 256],
            };
            for &v in head {
                self.put_u16(v);
            }
            for &n in &idx {
                if f == 0 {
                    self.put_u8(if n < 256 { i64::from(n) } else { 0 });
                } else {
                    self.put_u16(i64::from(n));
                }
            }
        }
        self.set_chksm(t);
        Ok(())
    }

    fn write_dirtab(&mut self, subsetted: bool) {
        let save_offset = self.opos;
        self.seek_out(TABDIR_OFF);
        let tabs: Vec<DirTab> = if subsetted {
            NEW_TAB_NAMES
                .iter()
                .filter_map(|n| self.lookup(**n).map(|t| self.dir[t]))
                .collect()
        } else {
            self.dir.clone()
        };
        for t in tabs {
            for c in t.tag {
                self.put_u8(i64::from(c));
            }
            self.put_u32(i64::from(t.checksum));
            self.put_u32(i64::from(t.offset));
            self.put_u32(i64::from(t.length));
        }
        // adjust checkSumAdjustment
        let mut tmp: u32 = 0;
        let mut sum: u32 = 0;
        let mut i = 0usize;
        for &b in &self.out[..save_offset] {
            tmp = (tmp << 8).wrapping_add(u32::from(b));
            i += 1;
            if i.is_multiple_of(4) {
                sum = sum.wrapping_add(tmp);
                tmp = 0;
            }
        }
        if !i.is_multiple_of(4) {
            sum <<= 8 * (4 - i % 4);
        }
        let k = 0xB1B0_AFBAu32.wrapping_sub(sum);
        self.seek_out(self.check_sum_adjustment_offset);
        self.put_u32(i64::from(k));
        self.seek_out(save_offset);
    }

    /// `ttf_init_font`.
    fn init_font(&mut self, n: u16) {
        let n = i64::from(n);
        let (mut i, mut k) = (1i64, 0i64);
        while i <= n {
            i <<= 1;
            k += 1;
        }
        self.put_u32(0x0001_0000);
        self.put_u16(n);
        self.put_u16(i << 3);
        self.put_u16(k - 1);
        self.put_u16((n << 4) - (i << 3));
        self.seek_out(TABDIR_OFF + usize::try_from(n).unwrap_or(0) * 4 * 4);
    }

    fn subset_font(&mut self, fd: &FontDesc, glyph_names: bool) -> Result<(), PdfError> {
        self.init_font(self.new_ntabs);
        for t in [b"PCLT", b"fpgm", b"cvt ", b"prep"] {
            if self.lookup(*t).is_some() {
                self.copytab(*t)?;
            }
        }
        self.reindex_glyphs()?;
        self.write_glyf()?;
        self.write_loca()?;
        self.write_os2()?;
        self.write_head(true)?;
        self.write_hhea()?;
        self.write_hmtx()?;
        self.write_maxp()?;
        self.write_name(fd)?;
        self.write_post(glyph_names)?;
        self.write_cmap()?;
        self.write_dirtab(true);
        Ok(())
    }

    fn copy_font(&mut self) -> Result<(), PdfError> {
        self.init_font(self.ntabs);
        for i in 0..self.dir.len() {
            let tag = self.dir[i].tag;
            if &tag == b"head" {
                self.write_head(false)?;
            } else {
                self.copytab(tag)?;
            }
        }
        self.write_dirtab(false);
        Ok(())
    }
}

/// `writettf`: the font file to embed (its length is `ttf_length`), and
/// the font descriptor's values read from it. `names` are the glyph
/// names of the font's encoding.
///
/// # Errors
/// A subset font without an encoding (subfonts are not supported), or
/// a broken font file.
pub(crate) fn write_ttf(
    data: &[u8],
    fm: &MapEntry,
    fd: &mut FontDesc,
    names: Option<&GlyphNames>,
    glyph_names: bool,
    tags: &mut BTreeSet<[u8; 6]>,
) -> Result<Vec<u8>, PdfError> {
    let subsetted = fm.is_subsetted();
    if subsetted && names.is_none() {
        return Err(fail("Subset TrueType must be a reencoded or a subfont"));
    }
    let mut t = Ttf {
        data,
        pos: 0,
        out: Vec::new(),
        opos: 0,
        checksum: 0,
        tab_length: 0,
        tmp_ulong: 0,
        ntabs: 0,
        upem: 0,
        post_format: 0,
        loca_format: 0,
        last_glyf_offset: 0,
        glyphs_count: 0,
        new_glyphs_count: 2,
        nhmtxs: 0,
        new_ntabs: DEFAULT_NTABS,
        glyphs: Vec::new(),
        glyph_index: Vec::new(),
        names: Vec::new(),
        name_buf: Vec::new(),
        dir: Vec::new(),
        check_sum_adjustment_offset: 0,
        enc: Vec::new(),
        cmap: None,
        cmap_read: false,
    };
    // a TrueType collection: its first font
    let ttc = fm
        .ff_name
        .as_deref()
        .is_some_and(|n| n.len() >= 4 && n[n.len() - 4..].eq_ignore_ascii_case(b".ttc"));
    if ttc {
        if t.get_u32()? == 0x7474_6366 {
            t.skip(4 + 4)?;
            t.pos = t.get_u32()? as usize;
        } else {
            t.pos = 0;
        }
    }
    t.read_font(fd)?;
    if subsetted {
        if let Some(names) = names {
            t.copy_encoding(names, fd, tags);
        }
        t.subset_font(fd, glyph_names)?;
    } else {
        t.copy_font()?;
    }
    t.out.truncate(t.opos);
    Ok(t.out)
}
