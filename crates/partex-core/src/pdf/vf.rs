//! pdfTeX §? (part 32e): virtual fonts in PDF output — `do_vf` reads a
//! `.vf` file the first time one of its characters is shipped, loading
//! its local fonts as TeX fonts; `do_vf_packet` plays a character's
//! packet back through the PDF page machinery.
//!
//! Packets are kept as pdfTeX stores them (local font selections
//! renumbered, `xxx` lengths in one byte), so playing them back is the
//! same byte walk.

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::host::{FileKind, Host};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;
use partex_engine::scaled::{Scaled, round_xn_over_d};

/// A virtual font: its local fonts (the file's number, the internal
/// font) and its character packets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Vf {
    fonts: Vec<(i32, i32)>,
    packets: Vec<Option<Arc<[u8]>>>,
    /// The hash of the two above (a state hash takes it: a font's packets
    /// are many, and never change).
    digest: u128,
}

partex_engine::persist_struct!(Vf {
    fonts,
    packets,
    digest
});

impl Vf {
    fn new(fonts: Vec<(i32, i32)>, packets: Vec<Option<Arc<[u8]>>>) -> Self {
        let digest = partex_engine::stablehash::StableHasher::of(&(&fonts, &packets));
        Self {
            fonts,
            packets,
            digest,
        }
    }
}

/// By content, through the digest.
impl core::hash::Hash for Vf {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.digest.hash(h);
    }
}

/// `pdf_font_type`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FontType {
    /// `new_font_type`: no `.vf` file looked for yet.
    #[default]
    New,
    /// `real_font_type`.
    Real,
    /// `virtual_font_type`.
    Virtual(Arc<Vf>),
}

partex_engine::persist_enum!(FontType { New, Real, Virtual(a0) });

const SET_CHAR_127: u8 = 127;
const SET1: u8 = 128;
const SET_RULE: u8 = 132;
const PUT1: u8 = 133;
const PUT_RULE: u8 = 137;
const NOP: u8 = 138;
const PUSH: u8 = 141;
const POP: u8 = 142;
const RIGHT1: u8 = 143;
const W0: u8 = 147;
const X0: u8 = 152;
const DOWN1: u8 = 157;
const Y0: u8 = 161;
const Z0: u8 = 166;
const FNT_NUM_0: u8 = 171;
const FNT1: u8 = 235;
const XXX1: u8 = 239;
const FNT_DEF1: u8 = 243;
const PRE: u8 = 247;
const POST: u8 = 248;
const LONG_CHAR: u8 = 242;
const VF_ID: u8 = 202;
const VF_MAX_PACKET_LENGTH: i32 = 10000;
const VF_MAX_RECURSION: i32 = 10;
const VF_STACK_SIZE: i32 = 100;

/// `store_scaled_f`: a `fix_word` times `z`, or `None` for pdfTeX's
/// "vf scaling" error.
fn store_scaled_f(sq: i32, z: Scaled) -> Option<Scaled> {
    let mut z = z;
    let mut alpha = 16;
    while z >= 0o40000000 {
        z /= 2;
        alpha += alpha;
    }
    let beta = 256 / alpha;
    let alpha = alpha * z;
    let u = if sq >= 0 {
        sq.cast_unsigned()
    } else {
        (sq + 0x4000_0000 + 0x4000_0000).cast_unsigned()
    };
    let [a, b, c, d] = u.to_be_bytes();
    let a = if sq >= 0 { a } else { a.wrapping_add(128) };
    let (b, c, d) = (i32::from(b), i32::from(c), i32::from(d));
    let sw = (((((d * z) / 0o400) + (c * z)) / 0o400) + (b * z)) / beta;
    match a {
        0 => Some(sw),
        255 => Some(sw - alpha),
        _ => None,
    }
}

/// A reader over the file with pdfTeX's error for running off its end.
struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bytes<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = self.data.get(self.pos).copied();
        self.pos += 1;
        b
    }
}

/// The ways reading a VF file stops.
enum Stop {
    Eof,
    Bad(&'static [u8]),
    Jump(Jump),
}

impl From<Jump> for Stop {
    fn from(j: Jump) -> Self {
        Self::Jump(j)
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    fn scale_f(&mut self, sq: i32, z: Scaled) -> Result<Scaled, Jump> {
        if z >= 0o1000000000 {
            return self.pdf_error(b"font", b"size is too large");
        }
        match store_scaled_f(sq, z) {
            Some(s) => Ok(s),
            None => self.pdf_error(b"store_scaled_f", b"vf scaling"),
        }
    }

    /// The font type of `f`, reading its `.vf` file the first time (a
    /// write of the `PDF_FONTS` table).
    pub(crate) fn pdf_font_type(&mut self, f: i32) -> Result<FontType, Jump> {
        use super::val::{bit, field::PDF_FONTS};
        let t = &self.pdf_font_ref(f).font_type;
        if *t != FontType::New {
            return Ok(t.clone());
        }
        self.writer_scope(0, bit(PDF_FONTS), |t| {
            if t.pdf_font(f).font_type == FontType::New {
                t.do_vf(f)?;
            }
            Ok(t.pdf_font(f).font_type.clone())
        })
    }

    /// `do_vf`.
    fn do_vf(&mut self, f: i32) -> Result<(), Jump> {
        self.pdf_font(f).font_type = FontType::Real;
        if self.auto_expand_vf(f)? {
            return Ok(()); // an auto-expanded virtual font
        }
        let mut name = self.font_name_bytes(f);
        name.extend_from_slice(b".vf");
        let found = self.host.read_file(&name, FileKind::Vf);
        if T::VALUES {
            self.tracker
                .load(&name, FileKind::Vf, found.as_ref().map(|f| &f.contents));
        }
        let Some(file) = found else {
            return Ok(());
        };
        let mut r = Bytes {
            data: &file.contents,
            pos: 0,
        };
        match self.read_vf(f, &mut r) {
            Ok(vf) => {
                self.pdf_font(f).font_type = FontType::Virtual(Arc::new(vf));
                Ok(())
            }
            Err(Stop::Jump(j)) => Err(j),
            Err(Stop::Eof) => self.pdf_error(b"vf", b"unexpected EOF or error"),
            Err(Stop::Bad(m)) => self.pdf_error(&name, m),
        }
    }

    fn read_vf(&mut self, f: i32, r: &mut Bytes<'_>) -> Result<Vf, Stop> {
        let byte = |r: &mut Bytes<'_>| r.byte().ok_or(Stop::Eof);
        let signed = |r: &mut Bytes<'_>, k: u8| -> Result<i32, Stop> {
            let mut i = i32::from(byte(r)?);
            if i >= 128 {
                i -= 256;
            }
            for _ in 1..k {
                i = i.wrapping_mul(256) + i32::from(byte(r)?);
            }
            Ok(i)
        };
        let unsigned = |r: &mut Bytes<'_>, k: u8| -> Result<i32, Stop> {
            let mut i = i32::from(byte(r)?);
            if k == 4 && i >= 128 {
                return Err(Stop::Bad(b"number too big"));
            }
            for _ in 1..k {
                i = i.wrapping_mul(256) + i32::from(byte(r)?);
            }
            Ok(i)
        };
        // the preamble
        if byte(r)? != PRE {
            return Err(Stop::Bad(b"PRE command expected"));
        }
        if byte(r)? != VF_ID {
            return Err(Stop::Bad(b"wrong id byte"));
        }
        let n = byte(r)?;
        for _ in 0..n {
            byte(r)?;
        }
        let cs = [byte(r)?, byte(r)?, byte(r)?, byte(r)?];
        let font = self.fonts.get(f);
        let (check, dsize, size) = (font.check, font.design_size, font.size);
        if cs != [0; 4] && check != [0; 4] && cs != check {
            self.print_nl(b"checksum mismatch in font ");
            self.print_str(&self.font_name_bytes(f));
            self.print_str(b".vf ignored");
        }
        if signed(r, 4)? / 0o20 != dsize {
            self.print_nl(b"design size mismatch in font ");
            self.print_str(&self.font_name_bytes(f));
            self.print_str(b".vf ignored");
        }
        self.update_terminal();
        // the font definitions
        let mut cmd = byte(r)?;
        let mut fonts = Vec::new();
        while (FNT_DEF1..FNT_DEF1 + 4).contains(&cmd) {
            let e = unsigned(r, cmd - FNT_DEF1 + 1)?;
            let k = self.vf_def_font(f, size, r)?;
            fonts.push((e, k));
            cmd = byte(r)?;
        }
        let mut packets = vec![None; 256];
        // the character packets
        while cmd <= LONG_CHAR {
            let (mut packet_length, cc, tfm_width);
            if cmd == LONG_CHAR {
                packet_length = unsigned(r, 4)?;
                cc = unsigned(r, 4)?;
                if !self.vf_valid_char(f, cc) {
                    return Err(Stop::Bad(b"invalid character code"));
                }
                let w = signed(r, 4)?;
                tfm_width = self.scale_f(w, size)?;
            } else {
                packet_length = i32::from(cmd);
                cc = i32::from(byte(r)?);
                if !self.vf_valid_char(f, cc) {
                    return Err(Stop::Bad(b"invalid character code"));
                }
                let w = unsigned(r, 3)?;
                tfm_width = self.scale_f(w, size)?;
            }
            if packet_length < 0 {
                return Err(Stop::Bad(b"negative packet length"));
            }
            if packet_length > VF_MAX_PACKET_LENGTH {
                return Err(Stop::Bad(b"packet length too long"));
            }
            if tfm_width != self.fonts.get(f).glyph(cc).map_or(0, |g| g.width) {
                self.print_nl(b"character width mismatch in font ");
                self.print_str(&self.font_name_bytes(f));
                self.print_str(b".vf ignored");
            }
            let mut p: Vec<u8> = Vec::new();
            let mut stack_level = 0;
            while packet_length > 0 {
                let mut cmd = byte(r)?;
                packet_length -= 1;
                let mut cmd_length: i32;
                if cmd <= SET_CHAR_127 {
                    cmd_length = 0;
                } else if (FNT_NUM_0..FNT_NUM_0 + 64).contains(&cmd)
                    || (FNT1..FNT1 + 4).contains(&cmd)
                {
                    let k = if cmd >= FNT1 {
                        let k = unsigned(r, cmd - FNT1 + 1)?;
                        packet_length -= i32::from(cmd - FNT1 + 1);
                        k
                    } else {
                        i32::from(cmd - FNT_NUM_0)
                    };
                    if k >= 256 {
                        return Err(Stop::Bad(b"too many local fonts"));
                    }
                    if !fonts.iter().any(|&(e, _)| e == k) {
                        return Err(Stop::Bad(b"undefined local font"));
                    }
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "k < 256"
                    )]
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
                            let l = unsigned(r, cmd - XXX1 + 1)?;
                            packet_length -= i32::from(cmd - XXX1 + 1);
                            if l > VF_MAX_PACKET_LENGTH {
                                return Err(Stop::Bad(b"packet length too long"));
                            }
                            if l < 0 {
                                return Err(Stop::Bad(b"string of negative length"));
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
                                return self
                                    .overflow(b"virtual font stack size", VF_STACK_SIZE)
                                    .map_err(Stop::from);
                            }
                            stack_level += 1;
                            0
                        }
                        POP => {
                            if stack_level == 0 {
                                return Err(Stop::Bad(b"more POPs than PUSHs in character"));
                            }
                            stack_level -= 1;
                            0
                        }
                        _ => return Err(Stop::Bad(b"improver DVI command")),
                    };
                }
                if cmd != NOP {
                    p.push(cmd);
                }
                packet_length -= cmd_length;
                while cmd_length > 0 {
                    cmd_length -= 1;
                    p.push(byte(r)?);
                }
            }
            if stack_level != 0 {
                return Err(Stop::Bad(b"more PUSHs than POPs in character packet"));
            }
            if packet_length != 0 {
                return Err(Stop::Bad(b"invalid packet length or DVI command in packet"));
            }
            packets[crate::input::ux(cc)] = Some(Arc::from(p));
            cmd = byte(r)?;
        }
        if cmd != POST {
            return Err(Stop::Bad(b"POST command expected"));
        }
        Ok(Vf::new(fonts, packets))
    }

    /// `is_valid_char` of font `f`.
    fn vf_valid_char(&self, f: i32, c: i32) -> bool {
        self.fonts.get(f).glyph(c).is_some()
    }

    /// `vf_def_font`: a local font, loaded as a TeX font.
    fn vf_def_font(&mut self, f: i32, size: Scaled, r: &mut Bytes<'_>) -> Result<i32, Stop> {
        let byte = |r: &mut Bytes<'_>| r.byte().ok_or(Stop::Eof);
        let cs = [byte(r)?, byte(r)?, byte(r)?, byte(r)?];
        let sig = |r: &mut Bytes<'_>| -> Result<i32, Stop> {
            let mut i = i32::from(byte(r)?);
            if i >= 128 {
                i -= 256;
            }
            for _ in 1..4 {
                i = i.wrapping_mul(256) + i32::from(byte(r)?);
            }
            Ok(i)
        };
        let s4 = sig(r)?;
        let fs = self.scale_f(s4, size)?;
        let ds = sig(r)? / 0o20;
        let (a, l) = (byte(r)?, byte(r)?);
        for _ in 0..a {
            byte(r)?;
        }
        self.str_room(usize::from(l))?;
        for _ in 0..l {
            let c = byte(r)?;
            self.append_char(c);
        }
        let s = self.make_string()?;
        let s = i32::try_from(s).unwrap_or(0);
        let mut k = self.tfm_lookup(s, fs);
        if k == NULL_FONT {
            let empty = self.pool_str(b"");
            k = self.read_font_info(NULL_CS, s, empty, fs)?;
        } else {
            // (a rebuild's shipout run again reads the VF again: its local
            // font, loaded by its older run or by a step gone, is loaded
            // here)
            self.found_font(k);
        }
        if k != NULL_FONT {
            let kf = self.fonts.get(k);
            let (check, dsize) = (kf.check, kf.design_size);
            if cs != [0; 4] && check != [0; 4] && cs != check {
                self.vf_local_font_warning(f, k, b"checksum mismatch");
            }
            if ds != dsize {
                self.vf_local_font_warning(f, k, b"design size mismatch");
            }
            let x = self.fonts.expand[crate::fonts::fx(f)];
            if x.step != 0 {
                let (stretch, shrink) = self.expand_limits(f);
                self.set_expand_params(k, x.auto, stretch, shrink, x.step, x.ratio)?;
            }
        }
        Ok(k)
    }

    /// `tfm_lookup`: a loaded font named `s` at size `fs` (any size if 0);
    /// `s` is flushed if one is found.
    pub(crate) fn tfm_lookup(&mut self, s: i32, fs: Scaled) -> i32 {
        // (a pool string found once: the search is linear)
        let copy_area = self.pool_str(b"///...");
        if self.font_cells {
            let b = alloc::sync::Arc::from(self.str_bytes(crate::input::ux(s)));
            self.font_touch(crate::fonts::FontTouch::Name(b, false));
        }
        // (the fonts' names, areas and sizes are the table's)
        self.font_table_read();
        for k in self.fonts.loaded_fonts() {
            let name = self.fonts.name[crate::fonts::fx(k)];
            let copied = self.fonts.area[crate::fonts::fx(k)] == copy_area;
            if !copied
                && self.str_eq_str(crate::input::ux(name), crate::input::ux(s))
                && (fs == 0 || self.fonts.get(k).size == fs)
            {
                self.flush_string();
                return k;
            }
        }
        NULL_FONT
    }

    /// pdfTeX's `letter_space_font`: font `f` spaced out by `e`
    /// thousandths of its quad, as a new font (from its TFM file, `u`'s)
    /// that is virtual: each character is `f`'s between half the space
    /// before and half after.
    pub(crate) fn letter_space_font(&mut self, u: i32, f: i32, e: i32) -> Result<i32, Jump> {
        use super::val::{bit, field::PDF_FONTS};
        self.writer_scope(0, bit(PDF_FONTS), |t| t.letter_space_font_now(u, f, e))
    }

    fn letter_space_font_now(&mut self, u: i32, f: i32, e: i32) -> Result<i32, Jump> {
        use crate::track::font::{METRICS, PARAMS};
        self.font_read(f, METRICS);
        let (name, size) = (self.fonts.name[crate::fonts::fx(f)], self.fonts.get(f).size);
        let empty = self.pool_str(b"");
        let k = self.read_font_info(u, name, empty, size)?;
        if self.scan_keyword(b"nolig")? {
            self.set_no_ligatures(k); // disable ligatures for letter-spaced fonts
        }
        if k == NULL_FONT {
            return Ok(k);
        }
        self.font_read(f, PARAMS);
        self.font_read(k, PARAMS);
        let quad_f = self.fonts.get(f).param(6);
        if self.fonts.get(k).param(6) == 0 && quad_f > 0 {
            self.fonts.params_mut(k)[5] = quad_f;
            self.font_wrote(k, PARAMS);
        }
        let quad = self.fonts.get(k).param(6);
        if quad == 0 {
            self.pdf_warning(
                b"\\letterspacefont",
                b"font has zero em size (\\fontdimen6)",
                true,
                true,
            );
        }
        let d = round_xn_over_d(quad, e, 1000);
        self.fonts.metrics_mut(k).widen(d);
        // (the metrics are no longer what the identity says)
        self.fonts.remade[crate::fonts::fx(k)] = true;
        self.font_wrote(k, METRICS);
        // append, e.g., '+100ls' to the font name
        let old_setting = self.selector();
        self.set_selector(crate::print::NEW_STRING);
        self.print(self.fonts.name[crate::fonts::fx(k)]);
        if e > 0 {
            self.print_char(b'+');
        }
        self.print_int(e);
        self.print_str(b"ls");
        self.set_selector(old_setting);
        let s = self.make_string()?;
        self.fonts.rename(k, i32::try_from(s).unwrap_or(0));
        self.font_wrote(k, METRICS);
        self.font_table_wrote();
        // the virtual font: half the space, the character, half the space
        let mut z = size;
        let mut alpha = 16;
        while z >= 0o40000000 {
            z /= 2;
            alpha += alpha;
        }
        let beta = 256 / alpha;
        alpha *= z;
        let mut w = round_xn_over_d(quad_f, e, 2000);
        let b0 = if w >= 0 {
            0
        } else {
            w += alpha;
            255
        };
        let mut r = w * beta;
        let b1 = r / z;
        r %= z;
        let b2 = if r == 0 {
            0
        } else {
            r *= 256;
            let b = r / z;
            r %= z;
            b
        };
        let b3 = if r == 0 { 0 } else { r * 256 / z };
        let right = [RIGHT1 + 3, byte(b0), byte(b1), byte(b2), byte(b3)];
        let mut packets = vec![None; 256];
        let font = self.fonts.get(k);
        for c in font.bc..=font.ec {
            let c = byte(c);
            let mut p = right.to_vec();
            if c < SET1 {
                p.push(c);
            } else {
                p.extend_from_slice(&[SET1, c]);
            }
            p.extend_from_slice(&right);
            packets[usize::from(c)] = Some(Arc::from(p));
        }
        let vf = Vf::new(vec![(0, f)], packets);
        self.pdf_font(k).font_type = FontType::Virtual(Arc::new(vf));
        Ok(k)
    }

    /// pdfTeX's `auto_expand_vf`: an auto-expanded font of a virtual
    /// font is virtual too, with the base font's packets and its local
    /// fonts expanded alike.
    fn auto_expand_vf(&mut self, f: i32) -> Result<bool, Jump> {
        let x = self.fonts.expand[crate::fonts::fx(f)];
        if !x.auto || x.blink == NULL_FONT {
            return Ok(false);
        }
        let FontType::Virtual(base) = self.pdf_font_type(x.blink)? else {
            return Ok(false);
        };
        let mut fonts = Vec::with_capacity(base.fonts.len());
        for &(e, lf) in &base.fonts {
            let k = self.auto_expand_font(lf, x.ratio)?;
            self.copy_expand_params(k, lf, x.ratio);
            fonts.push((e, k));
        }
        let vf = Vf::new(fonts, base.packets.clone());
        self.pdf_font(f).font_type = FontType::Virtual(Arc::new(vf));
        Ok(true)
    }

    /// pdfTeX's `vf_expand_local_fonts`.
    pub(crate) fn vf_expand_local_fonts(&mut self, f: i32) -> Result<(), Jump> {
        let FontType::Virtual(vf) = self.pdf_font_ref(f).font_type.clone() else {
            return Ok(());
        };
        let x = self.fonts.expand[crate::fonts::fx(f)];
        let (stretch, shrink) = self.expand_limits(f);
        for &(_, lf) in &vf.fonts {
            self.set_expand_params(lf, x.auto, stretch, shrink, x.step, x.ratio)?;
            if matches!(self.pdf_font_ref(lf).font_type, FontType::Virtual(_)) {
                self.vf_expand_local_fonts(lf)?;
            }
        }
        Ok(())
    }

    fn vf_local_font_warning(&mut self, f: i32, k: i32, s: &[u8]) {
        self.print_nl(s);
        self.print_str(b" in local font ");
        self.print_str(&self.font_name_bytes(k));
        self.print_str(b" in virtual font ");
        self.print_str(&self.font_name_bytes(f));
        self.print_str(b".vf ignored.");
    }

    /// `output_one_char` at VF recursion level `depth`.
    pub(crate) fn pdf_output_char_at(&mut self, f: i32, c: u8, depth: i32) -> Result<(), Jump> {
        if let FontType::Virtual(vf) = self.pdf_font_type(f)? {
            return self.do_vf_packet(f, &vf, c, depth + 1);
        }
        self.draw(super::draw::Draw::Char { f, c })
    }

    /// The encoder's half of `output_one_char`: character `c` of font `f`
    /// (not a virtual one).
    pub(crate) fn emit_char(&mut self, f: i32, c: u8) -> Result<(), Jump> {
        self.pdf_begin_string(f)?;
        self.pdf_print_char(f, c);
        self.adv_char_width(f, c)?;
        Ok(())
    }

    /// `do_vf_packet`.
    fn do_vf_packet(&mut self, vf_f: i32, vf: &Vf, c: u8, depth: i32) -> Result<(), Jump> {
        if depth > VF_MAX_RECURSION {
            return self.overflow(b"max level recursion of virtual fonts", VF_MAX_RECURSION);
        }
        let (save_h, save_v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        let p: Arc<[u8]> = vf.packets[usize::from(c)]
            .clone()
            .unwrap_or_else(|| Arc::from([]));
        let size = self.fonts.get(vf_f).size;
        let mut f = vf.fonts.first().map_or(NULL_FONT, |&(_, k)| k);
        let (mut w, mut x, mut y, mut z) = (0, 0, 0, 0);
        let mut stack: Vec<[Scaled; 6]> = Vec::new();
        let mut i = 0;
        let next = |i: &mut usize| {
            let b = p.get(*i).copied().unwrap_or(0);
            *i += 1;
            b
        };
        let read = |i: &mut usize, k: u8, signed: bool, next: &dyn Fn(&mut usize) -> u8| {
            let mut v = i32::from(next(i));
            if signed && v >= 128 {
                v -= 256;
            }
            for _ in 1..k {
                v = v.wrapping_mul(256) + i32::from(next(i));
            }
            v
        };
        while i < p.len() {
            let cmd = next(&mut i);
            let (ch, char_move) = if cmd <= SET_CHAR_127 {
                (i32::from(cmd), true)
            } else if (FNT_NUM_0..FNT_NUM_0 + 64).contains(&cmd) || cmd == FNT1 {
                let k = if cmd == FNT1 {
                    i32::from(next(&mut i))
                } else {
                    i32::from(cmd - FNT_NUM_0)
                };
                match vf.fonts.iter().find(|&&(e, _)| e == k) {
                    Some(&(_, kf)) => f = kf,
                    None => return self.pdf_error(b"vf", b"local font not found"),
                }
                continue;
            } else {
                match cmd {
                    PUSH => {
                        let s = &self.pdf.ship;
                        stack.push([s.cur_h, s.cur_v, w, x, y, z]);
                        continue;
                    }
                    POP => {
                        let [h, v, w1, x1, y1, z1] = stack.pop().unwrap_or_default();
                        self.pdf.ship.cur_h = h;
                        self.pdf.ship.cur_v = v;
                        (w, x, y, z) = (w1, x1, y1, z1);
                        continue;
                    }
                    SET1..=131 => (read(&mut i, cmd - SET1 + 1, false, &next), true),
                    PUT1..=136 => (read(&mut i, cmd - PUT1 + 1, false, &next), false),
                    SET_RULE | PUT_RULE => {
                        let a = read(&mut i, 4, true, &next);
                        let ht = self.scale_f(a, size)?;
                        let b = read(&mut i, 4, true, &next);
                        let wd = self.scale_f(b, size)?;
                        if wd > 0 && ht > 0 {
                            let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
                            self.draw(super::draw::Draw::Rule {
                                x: h,
                                y: v,
                                w: wd,
                                h: ht,
                            })?;
                            if cmd == SET_RULE {
                                self.pdf.ship.cur_h += wd;
                            }
                        }
                        continue;
                    }
                    RIGHT1..=146 => {
                        let a = read(&mut i, cmd - RIGHT1 + 1, true, &next);
                        self.pdf.ship.cur_h += self.scale_f(a, size)?;
                        continue;
                    }
                    W0..=151 => {
                        if cmd > W0 {
                            let a = read(&mut i, cmd - W0, true, &next);
                            w = self.scale_f(a, size)?;
                        }
                        self.pdf.ship.cur_h += w;
                        continue;
                    }
                    X0..=156 => {
                        if cmd > X0 {
                            let a = read(&mut i, cmd - X0, true, &next);
                            x = self.scale_f(a, size)?;
                        }
                        self.pdf.ship.cur_h += x;
                        continue;
                    }
                    DOWN1..=160 => {
                        let a = read(&mut i, cmd - DOWN1 + 1, true, &next);
                        self.pdf.ship.cur_v += self.scale_f(a, size)?;
                        continue;
                    }
                    Y0..=165 => {
                        if cmd > Y0 {
                            let a = read(&mut i, cmd - Y0, true, &next);
                            y = self.scale_f(a, size)?;
                        }
                        self.pdf.ship.cur_v += y;
                        continue;
                    }
                    Z0..=170 => {
                        if cmd > Z0 {
                            let a = read(&mut i, cmd - Z0, true, &next);
                            z = self.scale_f(a, size)?;
                        }
                        self.pdf.ship.cur_v += z;
                        continue;
                    }
                    XXX1..=242 => {
                        let n = read(&mut i, cmd - XXX1 + 1, false, &next);
                        let mut s = Vec::new();
                        for _ in 0..n {
                            s.push(next(&mut i));
                        }
                        self.literal(&s, super::SCAN_SPECIAL, false)?;
                        continue;
                    }
                    _ => return self.pdf_error(b"vf", b"invalid DVI command"),
                }
            };
            // do_char
            match u8::try_from(ch).ok().filter(|_| self.vf_valid_char(f, ch)) {
                Some(c) => {
                    self.pdf_output_char_at(f, c, depth)?;
                    if char_move {
                        self.pdf.ship.cur_h += self.fonts.get(f).glyph(ch).map_or(0, |g| g.width);
                    }
                }
                None => self.char_warning(f, ch)?,
            }
        }
        self.pdf.ship.cur_h = save_h;
        self.pdf.ship.cur_v = save_v;
        Ok(())
    }
}

/// A byte of a packet pdfTeX builds.
fn byte(b: i32) -> u8 {
    u8::try_from(b & 0xff).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_fix_words() {
        // 1.0 (2^20) at 10pt is 10pt; -0.5 at 10pt is -5pt
        assert_eq!(store_scaled_f(1 << 20, 10 << 16), Some(10 << 16));
        assert_eq!(store_scaled_f(-(1 << 19), 10 << 16), Some(-(5 << 16)));
    }
}
