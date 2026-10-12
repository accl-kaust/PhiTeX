//! Virtual fonts at shipout (pdfTeX §708ff): `do_vf` asks the caller for
//! the font's `.vf` data, and `do_vf_packet` typesets a character's DVI
//! program.

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;

use crate::{FontSource, FontType, LiteralMode, PdfState, pdf_error};

const SET_CHAR_127: u8 = 127;
const SET1: u8 = 128;
const SET_RULE: u8 = 132;
const PUT1: u8 = 133;
const PUT_RULE: u8 = 137;
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
/// `vf_max_recursion`.
const VF_MAX_RECURSION: i32 = 10;

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

/// A packet being read.
struct Packet<'a> {
    p: &'a [u8],
    i: usize,
}

impl Packet<'_> {
    fn byte(&mut self) -> u8 {
        let b = self.p.get(self.i).copied().unwrap_or(0);
        self.i += 1;
        b
    }

    /// `packet_read_unsigned` and `packet_read_signed`.
    fn read(&mut self, k: u8, signed: bool) -> i32 {
        let mut v = i32::from(self.byte());
        if signed && v >= 128 {
            v -= 256;
        }
        for _ in 1..k {
            v = v.wrapping_mul(256).wrapping_add(i32::from(self.byte()));
        }
        v
    }
}

impl PdfState {
    /// `do_vf`: the font's type, from its virtual font if it has one.
    pub(crate) fn do_vf(&mut self, f: FontId, src: &dyn FontSource) {
        let vf = src.virtual_font(f);
        let st = self.font_mut(f);
        st.kind = FontType::Real;
        if let Some(v) = vf {
            st.vf = Some(v);
            st.kind = FontType::Virtual;
        }
    }

    /// `do_vf_packet`.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn do_vf_packet(&mut self, vf_f: FontId, c: u8, src: &dyn FontSource) {
        self.vf_cur_s += 1;
        if self.vf_cur_s > VF_MAX_RECURSION {
            self.fail(pdf_error(
                "",
                "TeX capacity exceeded, sorry [max level recursion of virtual fonts=10]",
            ));
            self.vf_cur_s -= 1;
            return;
        }
        let (save_h, save_v) = (self.cur_h, self.cur_v);
        let Some(vf) = self.font(vf_f).vf.clone() else {
            self.vf_cur_s -= 1;
            return;
        };
        let size = self.font(vf_f).metrics.size;
        let empty = Vec::new();
        let mut pk = Packet {
            p: vf.packets.get(&c).unwrap_or(&empty),
            i: 0,
        };
        let mut f = vf.fonts.first().map_or(0, |&(_, k)| k);
        let (mut w, mut x, mut y, mut z) = (0, 0, 0, 0);
        let mut stack: Vec<[Scaled; 6]> = Vec::new();
        macro_rules! scaled {
            ($k:expr) => {{
                let a = pk.read($k, true);
                match store_scaled_f(a, size) {
                    Some(v) => v,
                    None => {
                        self.fail(pdf_error("store_scaled_f", "vf scaling"));
                        0
                    }
                }
            }};
        }
        while pk.i < pk.p.len() {
            let cmd = pk.byte();
            let (ch, char_move) = if cmd <= SET_CHAR_127 {
                (i32::from(cmd), true)
            } else if (FNT_NUM_0..FNT_NUM_0 + 64).contains(&cmd) || cmd == FNT1 {
                let k = if cmd == FNT1 {
                    u32::from(pk.byte())
                } else {
                    u32::from(cmd - FNT_NUM_0)
                };
                match vf.fonts.iter().find(|&&(e, _)| e == k) {
                    Some(&(_, kf)) => f = kf,
                    None => self.fail(pdf_error("vf", "local font not found")),
                }
                continue;
            } else {
                match cmd {
                    PUSH => {
                        stack.push([self.cur_h, self.cur_v, w, x, y, z]);
                        continue;
                    }
                    POP => {
                        let [h, v, w1, x1, y1, z1] = stack.pop().unwrap_or_default();
                        self.cur_h = h;
                        self.cur_v = v;
                        (w, x, y, z) = (w1, x1, y1, z1);
                        continue;
                    }
                    SET1..=131 => (pk.read(cmd - SET1 + 1, false), true),
                    PUT1..=136 => (pk.read(cmd - PUT1 + 1, false), false),
                    SET_RULE | PUT_RULE => {
                        let ht = scaled!(4);
                        let wd = scaled!(4);
                        if wd > 0 && ht > 0 {
                            self.set_rule(self.cur_h, self.cur_v, wd, ht);
                            if cmd == SET_RULE {
                                self.cur_h += wd;
                            }
                        }
                        continue;
                    }
                    RIGHT1..=146 => {
                        self.cur_h += scaled!(cmd - RIGHT1 + 1);
                        continue;
                    }
                    W0..=151 => {
                        if cmd > W0 {
                            w = scaled!(cmd - W0);
                        }
                        self.cur_h += w;
                        continue;
                    }
                    X0..=156 => {
                        if cmd > X0 {
                            x = scaled!(cmd - X0);
                        }
                        self.cur_h += x;
                        continue;
                    }
                    DOWN1..=160 => {
                        self.cur_v += scaled!(cmd - DOWN1 + 1);
                        continue;
                    }
                    Y0..=165 => {
                        if cmd > Y0 {
                            y = scaled!(cmd - Y0);
                        }
                        self.cur_v += y;
                        continue;
                    }
                    Z0..=170 => {
                        if cmd > Z0 {
                            z = scaled!(cmd - Z0);
                        }
                        self.cur_v += z;
                        continue;
                    }
                    XXX1..=242 => {
                        let n = pk.read(cmd - XXX1 + 1, false);
                        let s: Vec<u8> = (0..n).map(|_| pk.byte()).collect();
                        self.literal(&s, LiteralMode::ScanSpecial);
                        continue;
                    }
                    _ => {
                        self.fail(pdf_error("vf", "invalid DVI command"));
                        break;
                    }
                }
            };
            // do_char
            self.load_font(f, src);
            let Some(c) = u16::try_from(ch).ok().filter(|&c| self.font(f).valid(c)) else {
                // char_warning
                continue;
            };
            self.output_one_char(f, u8::try_from(c).unwrap_or(0), src);
            if char_move {
                self.cur_h += self.font(f).width(c);
            }
        }
        self.cur_h = save_h;
        self.cur_v = save_v;
        self.vf_cur_s -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::store_scaled_f;

    #[test]
    fn scales_fix_words() {
        // 1.0 and -0.5 of 10pt
        assert_eq!(store_scaled_f(1 << 20, 10 << 16), Some(10 << 16));
        assert_eq!(store_scaled_f(-(1 << 19), 10 << 16), Some(-(5 << 16)));
    }
}
