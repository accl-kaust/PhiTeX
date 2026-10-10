//! Virtual fonts in PDF output (pdftex.web part 32e): `do_vf` reads a
//! font's `.vf` file the first time one of its characters is shipped
//! ([`parse`], the fold's `F::do_vf` loading the local fonts as TeX
//! fonts); `do_vf_packet` plays a character's packet back in the page's
//! walk (`content.rs`).
//!
//! Packets are kept as pdfTeX stores them (local font selections
//! rewritten as `fnt_num_k` or `fnt1 k`, an `xxx`'s length in one byte),
//! so playing them back is pdfTeX's byte walk.

use std::sync::Arc;

use crate::tfm::Font;

pub const SET_CHAR_127: u8 = 127;
pub const SET1: u8 = 128;
pub const SET_RULE: u8 = 132;
pub const PUT1: u8 = 133;
pub const PUT_RULE: u8 = 137;
pub const NOP: u8 = 138;
pub const PUSH: u8 = 141;
pub const POP: u8 = 142;
pub const RIGHT1: u8 = 143;
pub const W0: u8 = 147;
pub const X0: u8 = 152;
pub const DOWN1: u8 = 157;
pub const Y0: u8 = 161;
pub const Z0: u8 = 166;
pub const FNT_NUM_0: u8 = 171;
pub const FNT1: u8 = 235;
pub const XXX1: u8 = 239;
const LONG_CHAR: u8 = 242;
const FNT_DEF1: u8 = 243;
const PRE: u8 = 247;
const POST: u8 = 248;
const VF_ID: u8 = 202;
const VF_MAX_PACKET_LENGTH: i32 = 10000;
/// `vf_max_recursion`, `vf_stack_size`.
pub const VF_MAX_RECURSION: u32 = 10;
const VF_STACK_SIZE: i32 = 100;

/// A local font as the file defines it (`vf_def_font`'s reading).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalDef {
    /// The file's number of it (`vf_e_fnts`).
    pub e: i32,
    pub name: String,
    /// Its size (`store_scaled_f` of the file's, by the virtual font's).
    pub fs: i32,
    pub checksum: [u8; 4],
    /// Its design size as the file says.
    pub ds: i32,
}

/// A `.vf` file read: its local fonts' definitions, its packets, the
/// messages pdfTeX prints to the log while reading it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub locals: Vec<LocalDef>,
    pub packets: Vec<Option<Arc<[u8]>>>,
    pub warnings: Vec<String>,
}

/// A virtual font: its local fonts (the file's number, the font —
/// `None`: not loadable, `null_font`) and its character packets.
#[derive(Debug)]
pub struct Vf {
    pub fonts: Vec<(i32, Option<Arc<Font>>)>,
    pub packets: Vec<Option<Arc<[u8]>>>,
}

impl Vf {
    /// Character `c`'s packet (empty if the file has none).
    pub fn packet(&self, c: u8) -> Arc<[u8]> {
        self.packets.get(usize::from(c)).cloned().flatten().unwrap_or_else(|| Arc::from([]))
    }

    /// The local font numbered `k` by the file: `None` if there is none
    /// ("local font not found"), `Some(None)` if it was not loadable.
    pub fn local(&self, k: i32) -> Option<Option<Arc<Font>>> {
        self.fonts.iter().find(|(e, _)| *e == k).map(|(_, f)| f.clone())
    }

    /// The font a packet starts with (`vf_default_font`: the first local).
    pub fn default_font(&self) -> Option<Arc<Font>> {
        self.fonts.first().and_then(|(_, f)| f.clone())
    }
}

/// Equal by identity: a font's virtual font is read once per job.
impl PartialEq for Vf {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// `store_scaled_f`: a `fix_word` times `z`; `Err` with pdfTeX's message
/// (`font`, "size is too large"; `store_scaled_f`, "vf scaling").
pub fn store_scaled_f(sq: i32, z: i32) -> Result<i32, String> {
    if z >= 0o1_000_000_000 {
        return Err("pdfTeX error (font): size is too large".into());
    }
    let mut z = z;
    let mut alpha = 16;
    while z >= 0o40_000_000 {
        z /= 2;
        alpha += alpha;
    }
    let beta = 256 / alpha;
    let alpha = alpha * z;
    let u = if sq >= 0 { sq.cast_unsigned() } else { (sq + 0x4000_0000 + 0x4000_0000).cast_unsigned() };
    let [a, b, c, d] = u.to_be_bytes();
    let a = if sq >= 0 { a } else { a.wrapping_add(128) };
    let (b, c, d) = (i32::from(b), i32::from(c), i32::from(d));
    let sw = (((((d * z) / 0o400) + (c * z)) / 0o400) + (b * z)) / beta;
    match a {
        0 => Ok(sw),
        255 => Ok(sw - alpha),
        _ => Err("pdfTeX error (store_scaled_f): vf scaling".into()),
    }
}

/// The ways reading stops: pdfTeX's `pdf_error`s (fatal), as their text.
fn bad(name: &str, m: &str) -> String {
    format!("pdfTeX error ({name}.vf): {m}")
}

struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bytes<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = self.data.get(self.pos).copied();
        self.pos += 1;
        b.ok_or_else(|| "pdfTeX error (vf): unexpected EOF or error".to_string())
    }
    fn signed(&mut self, k: u8) -> Result<i32, String> {
        let mut i = i32::from(self.byte()?);
        if i >= 128 {
            i -= 256;
        }
        for _ in 1..k {
            i = i.wrapping_mul(256) + i32::from(self.byte()?);
        }
        Ok(i)
    }
    fn unsigned(&mut self, k: u8, name: &str) -> Result<i32, String> {
        let mut i = i32::from(self.byte()?);
        if k == 4 && i >= 128 {
            return Err(bad(name, "number too big"));
        }
        for _ in 1..k {
            i = i.wrapping_mul(256) + i32::from(self.byte()?);
        }
        Ok(i)
    }
}

/// `do_vf`'s reading of font `f`'s file `data` (pdftex.web "Process the
/// preamble", "Process the font definitions", "Build a character
/// packet"): `Err` is a fatal `pdf_error`'s text.
pub fn parse(f: &Font, data: &[u8]) -> Result<Parsed, String> {
    let name = f.name.as_str();
    let mut r = Bytes { data, pos: 0 };
    let mut warnings = Vec::new();
    if r.byte()? != PRE {
        return Err(bad(name, "PRE command expected"));
    }
    if r.byte()? != VF_ID {
        return Err(bad(name, "wrong id byte"));
    }
    let n = r.byte()?;
    for _ in 0..n {
        r.byte()?;
    }
    let cs = [r.byte()?, r.byte()?, r.byte()?, r.byte()?];
    let check = f.checksum.to_be_bytes();
    if cs != [0; 4] && check != [0; 4] && cs != check {
        warnings.push(format!("checksum mismatch in font {name}.vf ignored"));
    }
    if r.signed(4)? / 0o20 != f.design_size {
        warnings.push(format!("design size mismatch in font {name}.vf ignored"));
    }
    // ("Process the font definitions")
    let mut cmd = r.byte()?;
    let mut locals = Vec::new();
    while (FNT_DEF1..FNT_DEF1 + 4).contains(&cmd) {
        let e = r.unsigned(cmd - FNT_DEF1 + 1, name)?;
        let checksum = [r.byte()?, r.byte()?, r.byte()?, r.byte()?];
        let s4 = r.signed(4)?;
        let fs = store_scaled_f(s4, f.size)?;
        let ds = r.signed(4)? / 0o20;
        let (a, l) = (r.byte()?, r.byte()?);
        for _ in 0..a {
            r.byte()?;
        }
        let mut s = String::new();
        for _ in 0..l {
            s.push(char::from(r.byte()?));
        }
        locals.push(LocalDef { e, name: s, fs, checksum, ds });
        cmd = r.byte()?;
    }
    // ("Build a character packet")
    let mut packets = vec![None; 256];
    let valid = |c: i32| f.glyph(c).is_some();
    while cmd <= LONG_CHAR {
        let (mut packet_length, cc, tfm_width);
        if cmd == LONG_CHAR {
            packet_length = r.unsigned(4, name)?;
            cc = r.unsigned(4, name)?;
            if !valid(cc) {
                return Err(bad(name, "invalid character code"));
            }
            let w = r.signed(4)?;
            tfm_width = store_scaled_f(w, f.size)?;
        } else {
            packet_length = i32::from(cmd);
            cc = i32::from(r.byte()?);
            if !valid(cc) {
                return Err(bad(name, "invalid character code"));
            }
            let w = r.unsigned(3, name)?;
            tfm_width = store_scaled_f(w, f.size)?;
        }
        if packet_length < 0 {
            return Err(bad(name, "negative packet length"));
        }
        if packet_length > VF_MAX_PACKET_LENGTH {
            return Err(bad(name, "packet length too long"));
        }
        if tfm_width != f.glyph(cc).map_or(0, |g| g.width) {
            warnings.push(format!("character width mismatch in font {name}.vf ignored"));
        }
        let mut p: Vec<u8> = Vec::new();
        let mut stack_level = 0;
        while packet_length > 0 {
            let mut cmd = r.byte()?;
            packet_length -= 1;
            let mut cmd_length: i32;
            if cmd <= SET_CHAR_127 {
                cmd_length = 0;
            } else if (FNT_NUM_0..FNT_NUM_0 + 64).contains(&cmd) || (FNT1..FNT1 + 4).contains(&cmd) {
                let k = if cmd >= FNT1 {
                    let k = r.unsigned(cmd - FNT1 + 1, name)?;
                    packet_length -= i32::from(cmd - FNT1 + 1);
                    k
                } else {
                    i32::from(cmd - FNT_NUM_0)
                };
                if k >= 256 {
                    return Err(bad(name, "too many local fonts"));
                }
                if !locals.iter().any(|l| l.e == k) {
                    return Err(bad(name, "undefined local font"));
                }
                if k <= 63 {
                    p.push(FNT_NUM_0 + k as u8);
                } else {
                    p.push(FNT1);
                    p.push(k as u8);
                }
                cmd_length = 0;
                cmd = NOP;
            } else {
                cmd_length = match cmd {
                    SET_RULE | PUT_RULE => 8,
                    SET1..=131 => i32::from(cmd - SET1 + 1),
                    PUT1..=136 => i32::from(cmd - PUT1 + 1),
                    RIGHT1..=146 => i32::from(cmd - RIGHT1 + 1),
                    148..=151 => i32::from(cmd - 148 + 1),
                    153..=156 => i32::from(cmd - 153 + 1),
                    DOWN1..=160 => i32::from(cmd - DOWN1 + 1),
                    162..=165 => i32::from(cmd - 162 + 1),
                    167..=170 => i32::from(cmd - 167 + 1),
                    XXX1..=242 => {
                        let l = r.unsigned(cmd - XXX1 + 1, name)?;
                        packet_length -= i32::from(cmd - XXX1 + 1);
                        if l > VF_MAX_PACKET_LENGTH {
                            return Err(bad(name, "packet length too long"));
                        }
                        if l < 0 {
                            return Err(bad(name, "string of negative length"));
                        }
                        p.push(XXX1);
                        // (`append_char` keeps one byte)
                        p.push(l.to_le_bytes()[0]);
                        cmd = NOP;
                        l
                    }
                    W0 | X0 | Y0 | Z0 | NOP => 0,
                    PUSH => {
                        if stack_level == VF_STACK_SIZE {
                            return Err(format!("! TeX capacity exceeded, sorry [virtual font stack size={VF_STACK_SIZE}]."));
                        }
                        stack_level += 1;
                        0
                    }
                    POP => {
                        if stack_level == 0 {
                            return Err(bad(name, "more POPs than PUSHs in character"));
                        }
                        stack_level -= 1;
                        0
                    }
                    _ => return Err(bad(name, "improver DVI command")),
                };
            }
            if cmd != NOP {
                p.push(cmd);
            }
            packet_length -= cmd_length;
            while cmd_length > 0 {
                cmd_length -= 1;
                p.push(r.byte()?);
            }
        }
        if stack_level != 0 {
            return Err(bad(name, "more PUSHs than POPs in character packet"));
        }
        if packet_length != 0 {
            return Err(bad(name, "invalid packet length or DVI command in packet"));
        }
        packets[cc as usize] = Some(Arc::from(p));
        cmd = r.byte()?;
    }
    if cmd != POST {
        return Err(bad(name, "POST command expected"));
    }
    Ok(Parsed { locals, packets, warnings })
}

/// `vf_local_font_warning`.
pub fn local_warning(f: &Font, k: &Font, s: &str) -> String {
    format!("{s} in local font {} in virtual font {}.vf ignored.", k.name, f.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_fix_words() {
        // (1.0 at 10pt is 10pt; -0.5 at 10pt is -5pt)
        assert_eq!(store_scaled_f(1 << 20, 10 << 16), Ok(10 << 16));
        assert_eq!(store_scaled_f(-(1 << 19), 10 << 16), Ok(-(5 << 16)));
    }
}
