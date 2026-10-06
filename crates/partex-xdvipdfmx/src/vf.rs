//! vf.c, vf.h: virtual fonts.
//!
//! The loaded VFs are `self.vf.vf_fonts` (count = `len()`), indexed by the
//! id `vf_locate_font` returns. Character packets are `Rc<[u8]>` so that
//! `vf_set_char` can hold one while it runs the DVI interpreter (which may
//! load more VFs: C's "this code needs to be able to recurse").
//! Packet readers take the packet slice (ending at C's `end`) and a
//! position (C's `*start`).

use crate::pdfdev::Spt;
use crate::prelude::*;

/// `VF_ALLOC_SIZE`.
pub const VF_ALLOC_SIZE: u32 = 16;
/// `VF_ID`.
pub const VF_ID: u8 = 202;
/// `FIX_WORD_BASE`.
pub const FIX_WORD_BASE: f64 = 1048576.0;
/// `TEXPT2PT`.
pub const TEXPT2PT: f64 = 72.0 / 72.27;
/// `FW2PT`.
pub const FW2PT: f64 = TEXPT2PT / FIX_WORD_BASE;
/// `CHAR_INDEX_MIN`: characters from here are looked up via `idx_to_char`.
pub const CHAR_INDEX_MIN: i32 = 0x40000;

/// `struct font_def`: a font the VF uses.
#[derive(Clone, Debug, Default)]
pub struct FontDef {
    /// The id used inside the VF file.
    pub font_id: i32,
    pub checksum: u32,
    pub size: u32,
    pub design_size: u32,
    pub directory: Option<Vec<u8>>,
    pub name: Option<Vec<u8>>,
    /// Returned by the TFM module.
    pub tfm_id: i32,
    /// Returned by the DEV module (`dvi_locate_font`).
    pub dev_id: i32,
}

/// `struct vf`.
#[derive(Clone, Debug, Default)]
pub struct Vf {
    pub tex_name: Vec<u8>,
    pub ptsize: Spt,
    /// A fixword-pts quantity.
    pub design_size: u32,
    /// `dev_fonts` (`num_dev_fonts` = `len()`).
    pub dev_fonts: Vec<FontDef>,
    /// `max_dev_fonts`.
    pub max_dev_fonts: i32,
    /// `ch_pkt` (`num_chars` = `len()`).
    pub ch_pkt: Vec<Option<Rc<[u8]>>>,
    pub message_flag: u8,
    pub pkt_len: Vec<u32>,
    pub idx_to_char: Vec<u32>,
    pub max_idx: u32,
}

/// vf.c's globals.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `vf_fonts` (`num_vf_fonts` = `len()`).
    pub vf_fonts: Vec<Vf>,
    /// `max_vf_fonts`.
    pub max_vf_fonts: i32,
}

/// `resize_one_vf_font` (static).
fn resize_one_vf_font(a_vf: &mut Vf, size: u32) {
    let num_chars = a_vf.ch_pkt.len() as u32;
    let mut size = size;
    if size > num_chars {
        if size > 0x40000 {
            size = (size + 0x1000).max(num_chars + 0x40000);
        } else if size > 0x8000 {
            size = (size + 0x1000).max(num_chars + 0x8000);
        } else if size > 0x1000 {
            size = (size + 0x100).max(num_chars + 0x1000);
        } else {
            size = size.max(num_chars + 256);
        }
        a_vf.ch_pkt.resize(size as usize, None);
        a_vf.pkt_len.resize(size as usize, 0);
        a_vf.idx_to_char.resize(size as usize, u32::MAX);
    }
}

/// `unsigned_byte` (static).
fn unsigned_byte(s: &[u8], start: &mut usize) -> Result<i32> {
    let mut byte = 0;
    if *start < s.len() {
        byte = i32::from(s[*start]);
        *start += 1;
    } else {
        crate::fatal!("Premature end of DVI byte stream in VF font\n");
    }
    Ok(byte)
}

/// `get_pkt_signed_num` (static): `num + 1` bytes.
fn get_pkt_signed_num(s: &[u8], start: &mut usize, num: u8) -> Result<i32> {
    let mut val: i32 = 0;
    if s.len().saturating_sub(*start) > usize::from(num) {
        let mut next = || {
            let b = i32::from(s[*start]);
            *start += 1;
            b
        };
        val = next();
        if val > 0x7f {
            val -= 0x100;
        }
        if num >= 3 {
            val = (val << 8) | next();
        }
        if num >= 2 {
            val = (val << 8) | next();
        }
        if num >= 1 {
            val = (val << 8) | next();
        }
    } else {
        crate::fatal!("Premature end of DVI byte stream in VF font\n");
    }
    Ok(val)
}

/// `get_pkt_unsigned_num` (static).
fn get_pkt_unsigned_num(s: &[u8], start: &mut usize, num: u8) -> Result<i32> {
    let mut val: i32 = 0;
    if s.len().saturating_sub(*start) > usize::from(num) {
        let mut next = || {
            let b = i32::from(s[*start]);
            *start += 1;
            b
        };
        val = next();
        if num >= 3 {
            if val > 0x7f {
                val -= 0x100;
            }
            val = (val << 8) | next();
        }
        if num >= 2 {
            val = (val << 8) | next();
        }
        if num >= 1 {
            val = (val << 8) | next();
        }
    } else {
        crate::fatal!("Premature end of DVI byte stream in VF font\n");
    }
    Ok(val)
}

/// kpathsea's program name: dvipdfmx.c pretends to be `dvipdfmx`.
const KPSE_PROGNAME: &[u8] = b"dvipdfmx";

impl Dpx {
    /// `read_header` (static).
    fn read_header(&mut self, vf_file: &mut MemFile, thisfont: i32) -> Result<()> {
        if vf_file.get_unsigned_byte()? == crate::dvi::PRE && vf_file.get_unsigned_byte()? == VF_ID
        {
            // skip comment
            let n = vf_file.get_unsigned_byte()?;
            vf_file.skip_bytes(usize::from(n))?;
            // Skip checksum
            vf_file.skip_bytes(4)?;
            self.vf.vf_fonts[thisfont as usize].design_size =
                vf_file.get_positive_quad("VF", "design_size")?;
        } else {
            // C: fprintf(stderr, "VF file may be corrupt\n")
        }
        Ok(())
    }

    /// `resize_vf_fonts` (static): the new entries themselves are pushed
    /// by `vf_locate_font`.
    fn resize_vf_fonts(&mut self, size: i32) {
        if size > self.vf.max_vf_fonts {
            let extra = (size as usize).saturating_sub(self.vf.vf_fonts.len());
            self.vf.vf_fonts.reserve(extra);
            self.vf.max_vf_fonts = size;
        }
    }

    /// `read_a_char_def` (static).
    fn read_a_char_def(
        &mut self,
        vf_file: &mut MemFile,
        thisfont: i32,
        pkt_len: u32,
        ch: u32,
    ) -> Result<()> {
        let vf = &mut self.vf.vf_fonts[thisfont as usize];
        let mut idx = ch;
        if ch >= CHAR_INDEX_MIN as u32 {
            if vf.max_idx == 0 {
                vf.max_idx = CHAR_INDEX_MIN as u32;
            }
            idx = vf.max_idx;
        }
        // Resize and initialize character arrays if necessary
        if idx as usize >= vf.ch_pkt.len() {
            resize_one_vf_font(vf, idx + 1);
        }
        if ch >= CHAR_INDEX_MIN as u32 {
            if idx > CHAR_INDEX_MIN as u32 && vf.idx_to_char[(idx - 1) as usize] >= ch {
                crate::fatal!("Unexpected character code in vf file\n");
            }
            vf.idx_to_char[idx as usize] = ch;
            vf.max_idx += 1;
        }
        if pkt_len > 0 {
            let pkt = vf_file.read(pkt_len as usize);
            if pkt.len() != pkt_len as usize {
                crate::fatal!("VF file ended prematurely.");
            }
            vf.ch_pkt[idx as usize] = Some(Rc::from(pkt));
        }
        vf.pkt_len[idx as usize] = pkt_len;
        Ok(())
    }

    /// `read_a_font_def` (static).
    fn read_a_font_def(
        &mut self,
        vf_file: &mut MemFile,
        font_id: i32,
        thisfont: i32,
    ) -> Result<()> {
        let t = thisfont as usize;
        {
            let vf = &mut self.vf.vf_fonts[t];
            if vf.dev_fonts.len() as i32 >= vf.max_dev_fonts {
                vf.max_dev_fonts += VF_ALLOC_SIZE as i32;
                let extra = (vf.max_dev_fonts as usize).saturating_sub(vf.dev_fonts.len());
                vf.dev_fonts.reserve(extra);
            }
        }
        let mut dev_font = FontDef {
            font_id,
            ..FontDef::default()
        };
        dev_font.checksum = vf_file.get_unsigned_quad()?;
        dev_font.size = vf_file.get_positive_quad("VF", "font_size")?;
        dev_font.design_size = vf_file.get_positive_quad("VF", "font_design_size")?;
        let dir_length = usize::from(vf_file.get_unsigned_byte()?);
        let name_length = usize::from(vf_file.get_unsigned_byte()?);
        // fread: short reads are not errors; the strings end at a NUL.
        let mut directory = vf_file.read(dir_length).to_vec();
        let mut name = vf_file.read(name_length).to_vec();
        if let Some(z) = directory.iter().position(|&c| c == 0) {
            directory.truncate(z);
        }
        if let Some(z) = name.iter().position(|&c| c == 0) {
            name.truncate(z);
        }
        dev_font.directory = Some(directory);
        dev_font.name = Some(name.clone());
        let size = dev_font.size;
        self.vf.vf_fonts[t].dev_fonts.push(dev_font);
        let n = self.vf.vf_fonts[t].dev_fonts.len() - 1;
        let tfm_id = self.tfm_open(&name, true)?; // must exist
        self.vf.vf_fonts[t].dev_fonts[n].tfm_id = tfm_id;
        let ptsize = self.vf.vf_fonts[t].ptsize;
        let dev_id = self.dvi_locate_font(&name, crate::stream::sqxfw(ptsize, size as i32))?;
        self.vf.vf_fonts[t].dev_fonts[n].dev_id = dev_id;
        Ok(())
    }

    /// `process_vf_file` (static).
    fn process_vf_file(&mut self, vf_file: &mut MemFile, thisfont: i32) -> Result<()> {
        use crate::dvi::{FNT_DEF1, FNT_DEF4, POST};
        let mut eof = false;
        while !eof {
            let code = vf_file.get_unsigned_byte()?;
            match code {
                FNT_DEF1..=FNT_DEF4 => {
                    let font_id = vf_file.get_unsigned_num(code - FNT_DEF1)?;
                    self.read_a_font_def(vf_file, font_id, thisfont)?;
                }
                _ => {
                    if code < 242 {
                        // For a short packet, code is the pkt_len
                        let ch = u32::from(vf_file.get_unsigned_byte()?);
                        // Skip over TFM width since we already know it
                        vf_file.skip_bytes(3)?;
                        self.read_a_char_def(vf_file, thisfont, u32::from(code), ch)?;
                        continue;
                    }
                    if code == 242 {
                        let pkt_len = vf_file.get_positive_quad("VF", "pkt_len")?;
                        let ch = vf_file.get_unsigned_quad()?;
                        // Skip over TFM width since we already know it
                        vf_file.skip_bytes(4)?;
                        if ch < 0x0100_0000 {
                            self.read_a_char_def(vf_file, thisfont, pkt_len, ch)?;
                        } else {
                            crate::fatal!(
                                "Long character (>24 bits) in VF file.\nI can't handle long characters!\n"
                            );
                        }
                        continue;
                    }
                    if code == POST {
                        eof = true;
                        continue;
                    }
                    // C: fprintf(stderr, "Quitting on code=%d\n", code)
                    eof = true;
                }
            }
        }
        Ok(())
    }

    /// `vf_locate_font`: the vf id, or -1. `kpse_find_file`'s
    /// `must_exist` is the host's business.
    pub fn vf_locate_font(&mut self, tex_name: &[u8], ptsize: Spt) -> Result<i32> {
        let mut thisfont = -1;
        // Has this name and ptsize already been loaded as a VF?
        let found = self
            .vf
            .vf_fonts
            .iter()
            .position(|v| v.tex_name == tex_name && v.ptsize == ptsize);
        if let Some(i) = found {
            thisfont = i as i32;
        } else {
            // It hasn't already been loaded as a VF, so try to load it
            let mut full_vf_file_name =
                self.files
                    .find(tex_name, crate::io::Format::Vf, KPSE_PROGNAME);
            if full_vf_file_name.is_none() {
                full_vf_file_name =
                    self.files
                        .find(tex_name, crate::io::Format::Ovf, KPSE_PROGNAME);
            }
            if let Some(path) = full_vf_file_name {
                if let Some(data) = self.files.read(&path) {
                    let mut vf_file = MemFile::new(data, &path);
                    let num_vf_fonts = self.vf.vf_fonts.len() as i32;
                    if num_vf_fonts >= self.vf.max_vf_fonts {
                        self.resize_vf_fonts(self.vf.max_vf_fonts + VF_ALLOC_SIZE as i32);
                    }
                    thisfont = num_vf_fonts;
                    // Initialize some pointers and such
                    self.vf.vf_fonts.push(Vf {
                        tex_name: tex_name.to_vec(),
                        ptsize,
                        ..Vf::default()
                    });
                    self.read_header(&mut vf_file, thisfont)?;
                    self.process_vf_file(&mut vf_file, thisfont)?;
                }
            }
        }
        Ok(thisfont)
    }

    /// `vf_putrule` (static).
    fn vf_putrule(&mut self, s: &[u8], start: &mut usize, ptsize: Spt) -> Result<()> {
        let height = get_pkt_signed_num(s, start, 3)?;
        let width = get_pkt_signed_num(s, start, 3)?;
        let w = crate::stream::sqxfw(ptsize, width);
        let h = crate::stream::sqxfw(ptsize, height);
        self.dvi_rule(w, h)?;
        Ok(())
    }

    /// `vf_setrule` (static).
    fn vf_setrule(&mut self, s: &[u8], start: &mut usize, ptsize: Spt) -> Result<()> {
        let height = get_pkt_signed_num(s, start, 3)?;
        let s_width = crate::stream::sqxfw(ptsize, get_pkt_signed_num(s, start, 3)?);
        let h = crate::stream::sqxfw(ptsize, height);
        self.dvi_rule(s_width, h)?;
        self.dvi_right(s_width);
        Ok(())
    }

    /// `vf_fnt` (static).
    fn vf_fnt(&mut self, font_id: i32, vf_font: i32) {
        let vf = &self.vf.vf_fonts[vf_font as usize];
        let found = vf.dev_fonts.iter().position(|d| d.font_id == font_id);
        if let Some(i) = found {
            // Font was found
            let dev_id = vf.dev_fonts[i].dev_id;
            self.dvi_set_font(dev_id);
        } else {
            // C: fprintf(stderr, "Font_id: %d not found in VF\n", font_id)
        }
    }

    /// `vf_xxx` (static): a special (or a `Warning:`) in a packet.
    fn vf_xxx(&mut self, len: i32, s: &[u8], start: &mut usize) -> Result<()> {
        let len_u = len as usize;
        if len_u <= s.len() && *start <= s.len() - len_u {
            let buffer = &s[*start..*start + len_u];
            let mut p = 0;
            while p < len_u && buffer[p] == b' ' {
                p += 1;
            }
            // Warning message from virtual font. (C's memcmp of 8 bytes
            // meets the NUL after the buffer before reading past it.)
            if buffer[p..].starts_with(b"Warning:") {
                crate::warn!("VF:{}", String::from_utf8_lossy(&buffer[p + 8..]));
            } else {
                self.dvi_do_special(buffer)?;
            }
        } else {
            crate::fatal!("Premature end of DVI byte stream in VF font.");
        }
        *start += len_u;
        Ok(())
    }

    /// `vf_set_char`.
    pub fn vf_set_char(&mut self, ch: i32, vf_font: i32) -> Result<()> {
        use crate::dvi::*;
        let mut default_font = -1;
        let mut idx: i32 = ch;
        if ch >= CHAR_INDEX_MIN {
            idx = -1;
            let vf = &self.vf.vf_fonts[vf_font as usize];
            let mut j = CHAR_INDEX_MIN;
            let mut k = vf.max_idx as i32;
            while j < k {
                let mid = j + (k - j) / 2;
                let ch0 = vf.idx_to_char[mid as usize] as i32;
                if ch0 < ch {
                    j = mid + 1;
                } else if ch0 > ch {
                    k = mid;
                } else {
                    idx = mid;
                    break;
                }
            }
        }
        if vf_font < self.vf.vf_fonts.len() as i32 {
            let v = vf_font as usize;
            // Initialize to the first font or -1 if undefined
            let ptsize = self.vf.vf_fonts[v].ptsize;
            if !self.vf.vf_fonts[v].dev_fonts.is_empty() {
                default_font = self.vf.vf_fonts[v].dev_fonts[0].dev_id;
            }
            self.dvi_vf_init(default_font)?;
            // C compares the int idx with the unsigned num_chars.
            let pkt = if (idx as u32 as usize) >= self.vf.vf_fonts[v].ch_pkt.len() || idx < 0 {
                None
            } else {
                self.vf.vf_fonts[v].ch_pkt[idx as usize].clone()
            };
            let (pkt, end): (Option<Rc<[u8]>>, usize) = match pkt {
                None => {
                    // (C dereferences the NULL font list of a VF without fonts)
                    let Some(df) = self.vf.vf_fonts[v].dev_fonts.first() else {
                        crate::fatal!("Invalid VF file: a character not in it, and no font.");
                    };
                    let tfm_id = df.tfm_id;
                    let is_jfm = self.tfm_is_jfm(tfm_id)?;
                    if is_jfm != 0
                        && ch < CHAR_INDEX_MIN
                        && i64::from(ch) <= i64::from(crate::tfm::JFM_LASTCHAR)
                        && self.conf.compat_mode != crate::ctx::CompatMode::Xdv
                    {
                        // fallback multibyte character for (u)pTeX
                        if self.conf.verbose_level == 1 && self.vf.vf_fonts[v].message_flag == 0 {
                            self.vf.vf_fonts[v].message_flag = 1;
                        }
                        self.dvi_set(ch)?;
                        self.dvi_vf_finish()?;
                        return Ok(());
                    }
                    // C: "Tried to set a nonexistent character in a virtual font"
                    (None, 0)
                }
                Some(p) => {
                    let end = self.vf.vf_fonts[v].pkt_len[idx as usize] as usize;
                    (Some(p), end)
                }
            };
            if let Some(pkt) = pkt {
                let s = &pkt[..end.min(pkt.len())];
                let mut start = 0usize;
                while start < s.len() {
                    let opcode = s[start];
                    start += 1;
                    match opcode {
                        SET1 | SET2 | SET3 => {
                            let c = get_pkt_unsigned_num(s, &mut start, opcode - SET1)?;
                            self.dvi_set(c)?;
                        }
                        SET4 => {
                            crate::fatal!(
                                "Multibyte (>24 bits) character in VF packet.\nI can't handle this!"
                            );
                        }
                        SET_RULE => self.vf_setrule(s, &mut start, ptsize)?,
                        PUT1 | PUT2 | PUT3 => {
                            let c = get_pkt_unsigned_num(s, &mut start, opcode - PUT1)?;
                            self.dvi_put(c)?;
                        }
                        PUT4 => {
                            crate::fatal!(
                                "Multibyte (>24 bits) character in VF packet.\nI can't handle this!"
                            );
                        }
                        PUT_RULE => self.vf_putrule(s, &mut start, ptsize)?,
                        NOP => {}
                        PUSH => self.dvi_push()?,
                        POP => self.dvi_pop()?,
                        RIGHT1..=RIGHT4 => {
                            let d = get_pkt_signed_num(s, &mut start, opcode - RIGHT1)?;
                            self.dvi_right(crate::stream::sqxfw(ptsize, d));
                        }
                        W0 => self.dvi_w0(),
                        W1..=W4 => {
                            let d = get_pkt_signed_num(s, &mut start, opcode - W1)?;
                            self.dvi_w(crate::stream::sqxfw(ptsize, d));
                        }
                        X0 => self.dvi_x0(),
                        X1..=X4 => {
                            let d = get_pkt_signed_num(s, &mut start, opcode - X1)?;
                            self.dvi_x(crate::stream::sqxfw(ptsize, d));
                        }
                        DOWN1..=DOWN4 => {
                            let d = get_pkt_signed_num(s, &mut start, opcode - DOWN1)?;
                            self.dvi_down(crate::stream::sqxfw(ptsize, d));
                        }
                        Y0 => self.dvi_y0(),
                        Y1..=Y4 => {
                            let d = get_pkt_signed_num(s, &mut start, opcode - Y1)?;
                            self.dvi_y(crate::stream::sqxfw(ptsize, d));
                        }
                        Z0 => self.dvi_z0(),
                        Z1..=Z4 => {
                            let d = get_pkt_signed_num(s, &mut start, opcode - Z1)?;
                            self.dvi_z(crate::stream::sqxfw(ptsize, d));
                        }
                        FNT1..=FNT4 => {
                            let f = get_pkt_signed_num(s, &mut start, opcode - FNT1)?;
                            self.vf_fnt(f, vf_font);
                        }
                        XXX1..=XXX4 => {
                            let len = get_pkt_unsigned_num(s, &mut start, opcode - XXX1)?;
                            if len < 0 {
                                crate::warn!("VF: Special with {} bytes???", len);
                            } else {
                                self.vf_xxx(len, s, &mut start)?;
                            }
                        }
                        PTEXDIR => {
                            let d = unsigned_byte(s, &mut start)?;
                            self.dvi_dirchg(d as u8);
                        }
                        _ => {
                            if opcode <= SET_CHAR_127 {
                                self.dvi_set(i32::from(opcode))?;
                            } else if (FNT_NUM_0..=FNT_NUM_63).contains(&opcode) {
                                self.vf_fnt(i32::from(opcode - FNT_NUM_0), vf_font);
                            } else {
                                crate::fatal!("Unexpected opcode in vf file\n");
                            }
                        }
                    }
                }
            }
            self.dvi_vf_finish()?;
        } else {
            crate::fatal!("Font not loaded\n");
        }
        Ok(())
    }

    /// `vf_close_all_fonts`.
    pub fn vf_close_all_fonts(&mut self) {
        self.vf.vf_fonts = Vec::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_numbers() {
        let s = [0xff, 0xfe, 0x01, 0x80];
        let mut p = 0;
        assert_eq!(get_pkt_signed_num(&s, &mut p, 0).unwrap(), -1);
        assert_eq!(p, 1);
        let mut p = 0;
        assert_eq!(get_pkt_signed_num(&s, &mut p, 1).unwrap(), -2);
        let mut p = 0;
        assert_eq!(get_pkt_unsigned_num(&s, &mut p, 1).unwrap(), 0xfffe);
        let mut p = 0;
        assert_eq!(
            get_pkt_unsigned_num(&s, &mut p, 3).unwrap(),
            0xfffe_0180_u32 as i32
        );
        let mut p = 1;
        assert_eq!(get_pkt_unsigned_num(&s, &mut p, 2).unwrap(), 0xfe0180);
        let mut p = 3;
        assert_eq!(unsigned_byte(&s, &mut p).unwrap(), 0x80);
    }

    #[test]
    fn resize() {
        let mut v = Vf::default();
        resize_one_vf_font(&mut v, 66);
        assert_eq!(v.ch_pkt.len(), 256);
        assert_eq!(v.idx_to_char[0], u32::MAX);
        resize_one_vf_font(&mut v, 0x40001);
        assert_eq!(v.ch_pkt.len(), 0x41001);
    }
}
