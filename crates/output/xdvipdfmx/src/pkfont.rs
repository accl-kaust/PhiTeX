//! pkfont.c: PK (bitmap) fonts as Type 3 fonts, each glyph an image
//! mask at 72 dpi.

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use crate::ctx::{Dpx, Result};
use crate::fmt::{Buf, round_acc, sprint_number};
use crate::io::Format;
use crate::obj::STREAM_COMPRESS;
use crate::{fatal, warn};

/// `PKFONT_DPI_DEFAULT`.
pub const PKFONT_DPI_DEFAULT: u32 = 600;

/// The name a host's finder is asked for a PK file (`kpse_find_glyph`):
/// `NAME.DPIpk` and the base resolution `kpse_init_prog` was given, as
/// pdfTeX's writer asks (`cmr10.600pk@600`; no mode, dvipdfmx gives
/// none).
#[must_use]
pub fn pk_request(name: &[u8], dpi: u32, bdpi: u32) -> Vec<u8> {
    let mut r = name.to_vec();
    r.extend_from_slice(format!(".{dpi}pk@{bdpi}").as_bytes());
    r
}

const PK_XXX1: u8 = 240;
const PK_XXX4: u8 = 243;
const PK_YYY: u8 = 244;
const PK_POST: u8 = 245;
const PK_NO_OP: u8 = 246;
const PK_PRE: u8 = 247;

/// `struct pk_header_`.
#[derive(Default)]
struct PkHeader {
    pkt_len: u32,
    chrcode: i32,
    wd: i32,
    bm_wd: u32,
    bm_ht: u32,
    bm_hoff: i32,
    bm_voff: i32,
    dyn_f: i32,
    run_color: i32,
}

/// A PK file read as `fgetc` and dpxutil's readers read it (past the end:
/// `EOF`, which the readers take as `0xff` bytes, as C's do after
/// `fgetc` returns -1 into an unsigned char).
struct Pk<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Pk<'_> {
    fn getc(&mut self) -> i32 {
        let c = self.d.get(self.pos).map_or(-1, |&b| i32::from(b));
        self.pos += 1;
        c
    }
    fn byte(&mut self) -> u32 {
        (self.getc() as u32) & 0xff
    }
    fn unsigned(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 8) | self.byte();
        }
        v
    }
    fn signed(&mut self, n: usize) -> i32 {
        let v = self.unsigned(n);
        let shift = 32 - 8 * n as u32;
        ((v << shift) as i32) >> shift
    }
    fn skip(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n);
    }
}

/// `pk_packed_num`.
fn pk_packed_num(np: &mut u32, dyn_f: i32, dp: &[u8], pl: u32) -> u32 {
    let mut i = *np;
    let get_nyb = |i: u32| -> i32 {
        let b = dp[(i / 2) as usize];
        i32::from(if i % 2 == 1 {
            b & 0x0f
        } else {
            (b >> 4) & 0x0f
        })
    };
    if i / 2 == pl {
        warn!("EOD reached while unpacking pk_packed_num.");
        return 0;
    }
    let mut nyb = get_nyb(i);
    i += 1;
    let mut nmbr: u32 = 0;
    if nyb == 0 {
        let mut j = 0;
        loop {
            if i / 2 == pl {
                warn!("EOD reached while unpacking pk_packed_num.");
                break;
            }
            nyb = get_nyb(i);
            i += 1;
            j += 1;
            if nyb != 0 {
                break;
            }
        }
        nmbr = nyb as u32;
        while j > 0 {
            j -= 1;
            if i / 2 == pl {
                warn!("EOD reached while unpacking pk_packed_num.");
                break;
            }
            nyb = get_nyb(i);
            i += 1;
            nmbr = nmbr.wrapping_mul(16).wrapping_add(nyb as u32);
        }
        nmbr = nmbr.wrapping_add(((13 - dyn_f) * 16 + dyn_f - 15) as u32);
    } else if nyb <= dyn_f {
        nmbr = nyb as u32;
    } else if nyb < 14 {
        if i / 2 == pl {
            warn!("EOD reached while unpacking pk_packed_num.");
            return 0;
        }
        nmbr = ((nyb - dyn_f - 1) * 16 + get_nyb(i) + dyn_f + 1) as u32;
        i += 1;
    }
    *np = i;
    nmbr
}

/// `fill_black_run`: mask image, black is a clear bit.
fn fill_black_run(dp: &mut [u8], left: u32, run_count: u32) -> u32 {
    const MASK: [u8; 8] = [127, 191, 223, 239, 247, 251, 253, 254];
    if run_count > 0 {
        for l in left..=left + run_count - 1 {
            dp[(l / 8) as usize] &= MASK[(l % 8) as usize];
        }
    }
    run_count
}

/// `pk_decode_packed`.
fn pk_decode_packed(
    out: &mut Vec<u8>,
    wd: u32,
    ht: u32,
    dyn_f: i32,
    run_color: i32,
    dp: &[u8],
    pl: u32,
) {
    let rowbytes = wd.div_ceil(8) as usize;
    let mut row = vec![0u8; rowbytes];
    let mut run_color = run_color;
    let (mut np, mut run_count) = (0u32, 0u32);
    let mut i = 0;
    while i < ht {
        let mut repeat_count = 0;
        row.fill(0xff);
        let mut rowbits_left = wd;
        if run_count > 0 {
            let nbits = rowbits_left.min(run_count);
            if run_color == 0 {
                rowbits_left -= fill_black_run(&mut row, 0, nbits);
            } else {
                rowbits_left -= nbits;
            }
            run_count -= nbits;
        }
        while np / 2 < pl && rowbits_left > 0 {
            let b = dp[(np / 2) as usize];
            let nyb = if np % 2 == 1 {
                b & 0x0f
            } else {
                (b >> 4) & 0x0f
            };
            if nyb == 14 {
                if repeat_count != 0 {
                    warn!("Second repeat count for this row!");
                }
                np += 1;
                repeat_count = pk_packed_num(&mut np, dyn_f, dp, pl);
            } else if nyb == 15 {
                if repeat_count != 0 {
                    warn!("Second repeat count for this row!");
                }
                np += 1;
                repeat_count = 1;
            } else {
                run_count = pk_packed_num(&mut np, dyn_f, dp, pl);
                let nbits = rowbits_left.min(run_count);
                run_color = i32::from(run_color == 0);
                run_count -= nbits;
                if run_color == 0 {
                    rowbits_left -= fill_black_run(&mut row, wd - rowbits_left, nbits);
                } else {
                    rowbits_left -= nbits;
                }
            }
        }
        out.extend_from_slice(&row);
        while i < ht && repeat_count > 0 {
            out.extend_from_slice(&row);
            repeat_count -= 1;
            i += 1;
        }
        i += 1;
    }
}

/// `pk_decode_bitmap`.
fn pk_decode_bitmap(out: &mut Vec<u8>, wd: u32, ht: u32, run_color: i32, dp: &[u8], pl: u32) {
    const MASK: [u8; 8] = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];
    if run_color != 0 {
        warn!("run_color != 0 for bitmap pk data?");
    } else if u64::from(pl) < (u64::from(wd) * u64::from(ht)).div_ceil(8) {
        warn!("Insufficient bitmap pk data.");
        return;
    }
    let rowbytes = wd.div_ceil(8) as usize;
    let mut row = vec![0u8; rowbytes];
    let mut j = 0;
    for i in 0..ht * wd {
        let c = dp[(i / 8) as usize] & MASK[(i % 8) as usize];
        if c == 0 {
            row[(j / 8) as usize] |= MASK[(j % 8) as usize];
        }
        j += 1;
        if j == wd {
            out.extend_from_slice(&row);
            row.fill(0);
            j = 0;
        }
    }
}

/// `read_pk_char_header`.
fn read_pk_char_header(h: &mut PkHeader, opcode: u8, fp: &mut Pk<'_>) -> bool {
    if opcode & 4 == 0 {
        h.pkt_len = (u32::from(opcode) & 3) << 8 | fp.byte();
        h.chrcode = fp.byte() as i32;
        h.wd = fp.unsigned(3) as i32;
        let _dx = fp.byte() << 16;
        h.bm_wd = fp.byte();
        h.bm_ht = fp.byte();
        h.bm_hoff = fp.signed(1);
        h.bm_voff = fp.signed(1);
        h.pkt_len = h.pkt_len.wrapping_sub(8);
    } else if opcode & 7 == 7 {
        h.pkt_len = fp.unsigned(4);
        h.chrcode = fp.signed(4);
        h.wd = fp.signed(4);
        let _dx = fp.signed(4);
        let _dy = fp.signed(4);
        h.bm_wd = fp.unsigned(4);
        h.bm_ht = fp.unsigned(4);
        h.bm_hoff = fp.signed(4);
        h.bm_voff = fp.signed(4);
        h.pkt_len = h.pkt_len.wrapping_sub(28);
    } else {
        h.pkt_len = (u32::from(opcode) & 3) << 16 | fp.unsigned(2);
        h.chrcode = fp.byte() as i32;
        h.wd = fp.unsigned(3) as i32;
        let _dx = fp.unsigned(2) << 16;
        h.bm_wd = fp.unsigned(2);
        h.bm_ht = fp.unsigned(2);
        h.bm_hoff = fp.signed(2);
        h.bm_voff = fp.signed(2);
        h.pkt_len = h.pkt_len.wrapping_sub(13);
    }
    h.dyn_f = i32::from(opcode / 16);
    h.run_color = i32::from(opcode & 8 != 0);
    if h.chrcode as u32 > 0xff {
        warn!(
            "Unable to handle long characters in PK files: code=0x{:04x}",
            h.chrcode
        );
        return false;
    }
    true
}

impl Dpx {
    /// `PKFont_set_dpi`.
    pub fn pkfont_set_dpi(&mut self, dpi: i32) -> Result<()> {
        if dpi <= 0 {
            fatal!("Invalid DPI: {dpi}\n");
        }
        self.font.pk_base_dpi = dpi as u32;
        Ok(())
    }

    /// `base_dpi` (`PKFONT_DPI_DEFAULT` until set).
    fn pk_base_dpi(&self) -> u32 {
        if self.font.pk_base_dpi == 0 {
            PKFONT_DPI_DEFAULT
        } else {
            self.font.pk_base_dpi
        }
    }

    /// `truedpi`: the base resolution scaled by the font's size over its
    /// TFM's design size.
    fn truedpi(&mut self, ident: &[u8], point_size: f64, bdpi: u32) -> Result<u32> {
        let tfm_id = self.tfm_open(ident, false)?;
        if tfm_id < 0 {
            return Ok(bdpi);
        }
        let design_size = self.tfm_get_design_size(tfm_id)?;
        if design_size <= 0.0 {
            warn!(
                "DESGIN_SIZE <= 0.0? (TFM=\"{}\")",
                alloc::string::String::from_utf8_lossy(ident)
            );
            return Ok(bdpi);
        }
        Ok(round_acc(
            f64::from(self.pk_base_dpi()) * point_size / design_size,
            1.0,
        ) as u32)
    }

    /// `dpx_open_pk_font_at`: kpathsea's glyph search (and mktexpk) for
    /// `ident` at `dpi`, a `.pfb` suffix dropped; the file's bytes.
    fn dpx_open_pk_font_at(&mut self, ident: &[u8], dpi: u32) -> Option<alloc::sync::Arc<[u8]>> {
        let mut name = ident;
        if let Some(k) = ident.iter().rposition(|&c| c == b'.')
            && ident[k..].eq_ignore_ascii_case(b".pfb")
        {
            name = &ident[..k];
        }
        let request = pk_request(name, dpi, self.pk_base_dpi());
        let path = self.files.0.find(&request, Format::Pk, b"dvipdfmx")?;
        self.files.0.read(&path)
    }

    /// `pdf_font_open_pkfont`.
    pub fn pdf_font_open_pkfont(
        &mut self,
        font_id: i32,
        ident: &[u8],
        index: i32,
        encoding_id: i32,
        embedding: i32,
        point_size: f64,
    ) -> Result<i32> {
        if ident.is_empty() || point_size <= 0.0 {
            return Ok(-1);
        }
        if embedding == 0 {
            warn!("Ignoring no-embed option for PK font");
        }
        if index != 0 {
            warn!("Ignoring font index option for PK font");
        }
        let dpi = self.truedpi(ident, point_size, self.pk_base_dpi())?;
        if self.dpx_open_pk_font_at(ident, dpi).is_none() {
            return Ok(-1);
        }
        self.font.fonts[font_id as usize].fontname = Some(ident.to_vec());
        if encoding_id >= 0 {
            self.pdf_encoding_used_by_type3(encoding_id)?;
            let _ = self.pdf_encoding_get_name(encoding_id)?;
            warn!("PK font is found for font but non built-in encoding is specified.");
            warn!(">> Assuming this is for glyph name assignment.");
        }
        Ok(0)
    }

    /// `create_pk_CharProc_stream`.
    fn create_pk_charproc_stream(
        &mut self,
        pkh: &PkHeader,
        chrwid: f64,
        pkt: &[u8],
    ) -> Result<crate::obj::Obj> {
        let llx = -pkh.bm_hoff;
        let lly = pkh.bm_voff - pkh.bm_ht as i32;
        let urx = pkh.bm_wd as i32 - pkh.bm_hoff;
        let ury = pkh.bm_voff;
        let stream = self.o.new_stream(STREAM_COMPRESS);
        let mut b = Buf(Vec::new());
        sprint_number(&mut b, chrwid);
        b.0.extend_from_slice(format!(" 0 {llx} {lly} {urx} {ury} d1\n").as_bytes());
        self.o.add_stream(stream, &b.0)?;
        if pkh.bm_wd != 0 && pkh.bm_ht != 0 && !pkt.is_empty() {
            let (w, h) = (pkh.bm_wd, pkh.bm_ht);
            let mut s = format!("q\n{w} 0 0 {h} {llx} {lly} cm\n").into_bytes();
            s.extend_from_slice(format!("BI\n/W {w}\n/H {h}\n/IM true\n/BPC 1\nID ").as_bytes());
            let pl = u32::try_from(pkt.len()).unwrap_or(u32::MAX);
            if pkh.dyn_f == 14 {
                pk_decode_bitmap(&mut s, w, h, pkh.run_color, pkt, pl);
            } else {
                pk_decode_packed(&mut s, w, h, pkh.dyn_f, pkh.run_color, pkt, pl);
            }
            s.extend_from_slice(b"\nEI\nQ");
            self.o.add_stream(stream, &s)?;
        }
        Ok(stream)
    }

    /// `pdf_font_load_pkfont`.
    #[allow(clippy::too_many_lines)]
    pub fn pdf_font_load_pkfont(&mut self, font_id: i32) -> Result<()> {
        let fi = font_id as usize;
        if self.font.fonts[fi].reference.is_none() {
            return Ok(());
        }
        let ident = self.font.fonts[fi].filename.clone().unwrap_or_default();
        let point_size = self.font.fonts[fi].point_size;
        let usedchars = self.font.fonts[fi]
            .usedchars
            .clone()
            .expect("usedchars")
            .to_vec();
        let encoding_id = self.font.fonts[fi].encoding_id;
        let enc_vec = if encoding_id < 0 {
            None
        } else {
            Some(self.pdf_encoding_get_encoding(encoding_id)?)
        };
        let name_of = |code: usize| -> Vec<u8> {
            match enc_vec
                .as_ref()
                .and_then(|e| e.get(code).cloned().flatten())
            {
                Some(n) => n,
                None => format!("x{code:02X}").into_bytes(),
            }
        };
        let base_dpi = self.pk_base_dpi();
        let dpi = self.truedpi(&ident, point_size, base_dpi)?;
        let Some(data) = self.dpx_open_pk_font_at(&ident, dpi) else {
            fatal!(
                "Could not find/open PK font file: {} (at {}dpi)",
                alloc::string::String::from_utf8_lossy(&ident),
                dpi
            );
        };
        let mut fp = Pk { d: &data, pos: 0 };
        let mut charavail = [false; 256];
        let mut widths = [0.0f64; 256];
        let charprocs = self.o.new_dict();
        let pix2charu = 72. * 1000. / f64::from(base_dpi) / point_size;
        let (mut llx, mut lly, mut urx, mut ury) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        loop {
            let opcode = fp.getc();
            if opcode < 0 || opcode == i32::from(PK_POST) {
                break;
            }
            let opcode = opcode as u8;
            if opcode < 240 {
                let mut pkh = PkHeader::default();
                if !read_pk_char_header(&mut pkh, opcode, &mut fp) {
                    fatal!("Error in reading PK character header.");
                } else if charavail[(pkh.chrcode & 0xff) as usize] {
                    warn!("More than two bitmap image for single glyph?");
                }
                let code = (pkh.chrcode & 0xff) as usize;
                if usedchars[code] == 0 {
                    fp.skip(pkh.pkt_len as usize);
                } else {
                    let charwidth = round_acc(
                        1000.0 * f64::from(pkh.wd) / (f64::from(1u32 << 20) * pix2charu),
                        0.1,
                    );
                    widths[code] = charwidth;
                    llx = llx.min(f64::from(-pkh.bm_hoff));
                    lly = lly.min(f64::from(pkh.bm_voff) - f64::from(pkh.bm_ht));
                    urx = urx.max(f64::from(pkh.bm_wd) - f64::from(pkh.bm_hoff));
                    ury = ury.max(f64::from(pkh.bm_voff));
                    let start = fp.pos.min(data.len());
                    let end = fp.pos.saturating_add(pkh.pkt_len as usize).min(data.len());
                    let pkt = data[start..end].to_vec();
                    fp.skip(pkh.pkt_len as usize);
                    if pkt.len() != pkh.pkt_len as usize {
                        fatal!(
                            "Only {} bytes PK packet read. (expected {} bytes)",
                            pkt.len(),
                            pkh.pkt_len
                        );
                    }
                    let charproc = self.create_pk_charproc_stream(&pkh, charwidth, &pkt)?;
                    let mut charname = name_of(code);
                    if enc_vec.is_some()
                        && enc_vec
                            .as_ref()
                            .and_then(|e| e.get(code).cloned().flatten())
                            .is_none()
                    {
                        warn!("\".notdef\" glyph used in font (code=0x{code:02x})");
                        charname = format!("x{code:02X}").into_bytes();
                    }
                    let r = self.o.ref_obj(charproc)?;
                    self.o.put(charprocs, &charname, r)?;
                    self.o.release(charproc)?;
                }
                charavail[code] = true;
            } else {
                match opcode {
                    PK_NO_OP => {}
                    PK_XXX1..=PK_XXX4 => {
                        let len = fp.unsigned(usize::from(opcode - PK_XXX1) + 1) as i32;
                        if len < 0 {
                            warn!("PK: Special with {len} bytes???");
                        } else {
                            fp.skip(len as usize);
                        }
                    }
                    PK_YYY => fp.skip(4),
                    PK_PRE => {
                        if fp.getc() == 89 {
                            let k = fp.byte() as usize;
                            fp.skip(k);
                            fp.skip(16);
                        } else {
                            fatal!(
                                "embed_pk_font: PK ID byte is incorrect.  Are you sure this is a PK file?"
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        for code in 0..256 {
            if usedchars[code] != 0 && !charavail[code] {
                warn!("Missing glyph code=0x{code:02x} in PK font");
            }
        }
        let fontdict = self.pdf_font_get_resource(font_id)?;
        let r = self.o.ref_obj(charprocs)?;
        self.o.put(fontdict, b"CharProcs", r)?;
        self.o.release(charprocs)?;
        let procset = self.o.new_dict();
        let a = self.o.new_array();
        let n = self.o.new_name(b"PDF");
        self.o.add_array(a, n)?;
        let n = self.o.new_name(b"ImageB");
        self.o.add_array(a, n)?;
        self.o.put(procset, b"ProcSet", a)?;
        self.o.put(fontdict, b"Resources", procset)?;
        // Encoding
        let a = self.o.new_array();
        let (mut prev, mut firstchar, mut lastchar) = (-2i32, 255i32, 0i32);
        for code in 0..256i32 {
            if usedchars[code as usize] != 0 {
                firstchar = firstchar.min(code);
                lastchar = lastchar.max(code);
                if code != prev + 1 {
                    let n = self.o.new_number(f64::from(code));
                    self.o.add_array(a, n)?;
                }
                let n = self.o.new_name(&name_of(code as usize));
                self.o.add_array(a, n)?;
                prev = code;
            }
        }
        if firstchar > lastchar {
            fatal!("Unexpected error: firstchar > lastchar ({firstchar} {lastchar})");
        }
        if enc_vec.is_none() {
            let encoding = self.o.new_dict();
            self.o.put_name(encoding, b"Type", b"Encoding")?;
            self.o.put(encoding, b"Differences", a)?;
            let r = self.o.ref_obj(encoding)?;
            self.o.put(fontdict, b"Encoding", r)?;
            self.o.release(encoding)?;
        } else {
            self.o.release(a)?;
        }
        let a = self.o.new_array();
        for v in [llx, lly, urx, ury] {
            let n = self.o.new_number(v);
            self.o.add_array(a, n)?;
        }
        self.o.put(fontdict, b"FontBBox", a)?;
        let a = self.o.new_array();
        for code in firstchar..=lastchar {
            let w = if usedchars[code as usize] != 0 {
                widths[code as usize]
            } else {
                0.0
            };
            let n = self.o.new_number(w);
            self.o.add_array(a, n)?;
        }
        let r = self.o.ref_obj(a)?;
        self.o.put(fontdict, b"Widths", r)?;
        self.o.release(a)?;
        let a = self.o.new_array();
        for v in [0.001 * pix2charu, 0.0, 0.0, 0.001 * pix2charu, 0.0, 0.0] {
            let n = self.o.new_number(v);
            self.o.add_array(a, n)?;
        }
        self.o.put(fontdict, b"FontMatrix", a)?;
        let n = self.o.new_number(f64::from(firstchar));
        self.o.put(fontdict, b"FirstChar", n)?;
        let n = self.o.new_number(f64::from(lastchar));
        self.o.put(fontdict, b"LastChar", n)?;
        Ok(())
    }
}
