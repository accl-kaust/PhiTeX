//! cmap_write.c, cmap_write.h: a CMap as a PDF stream.
//!
//! C's `#if 0` parts (`CMap_ToCode_stream`, `add_inverse_map`, `add_map`,
//! `invert_cmap`, `flatten_cmap`) are not ported.

#![allow(non_snake_case)]

use core::fmt::Write as _;

use crate::cmap::{
    CMAP_DEBUG_STR, CMAP_TYPE_IDENTITY, CMAP_TYPE_TO_UNICODE, CMap, LOOKUP_CONTINUE, MAP_DEFINED,
    MAP_IS_CID, MAP_IS_CODE, MAP_IS_NAME, MAP_IS_NOTDEF, MAP_TYPE, MapDef,
};
use crate::fmt::Buf;
use crate::obj::STREAM_COMPRESS;
use crate::prelude::*;

/// `BLOCK_LEN_MIN` (must be greater than 1).
pub const BLOCK_LEN_MIN: i32 = 2;
/// `WBUF_SIZE`.
pub const WBUF_SIZE: usize = 40960;

/// `CMAP_BEGIN`.
pub const CMAP_BEGIN: &[u8] = b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n";
/// `CMAP_END`.
pub const CMAP_END: &[u8] = b"endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";

/// `struct sbuf`: `buf` is `WBUF_SIZE` bytes, `curptr` and `limptr`
/// positions in it.
#[derive(Clone, Debug, Default)]
pub struct Sbuf {
    pub buf: Vec<u8>,
    pub curptr: usize,
    pub limptr: usize,
}

impl Sbuf {
    /// `*(wbuf->curptr)++ = c`.
    fn put(&mut self, c: u8) {
        self.buf[self.curptr] = c;
        self.curptr += 1;
    }
    /// `wbuf->curptr += sprintf(wbuf->curptr, ...)`.
    fn puts(&mut self, s: &[u8]) {
        self.buf[self.curptr..self.curptr + s.len()].copy_from_slice(s);
        self.curptr += s.len();
    }
    /// `sputx(c, &(wbuf->curptr), wbuf->limptr)`.
    fn putx(&mut self, c: u8) -> Result<()> {
        let end = self.limptr;
        sputx(c, &mut self.buf, &mut self.curptr, end)?;
        Ok(())
    }
    /// The bytes written (`wbuf->buf` to `wbuf->curptr`).
    fn written(&self) -> &[u8] {
        &self.buf[..self.curptr]
    }
}

/// `block_count`: how many consecutive entries from `c` form a block.
fn block_count(mtab: &[MapDef], c: i32) -> i32 {
    let mut count = 0;
    let n = (mtab[c as usize].len - 1) as usize;
    let mut c = c as usize + 1;
    while c < 256 {
        if LOOKUP_CONTINUE(mtab[c].flag)
            || !MAP_DEFINED(mtab[c].flag)
            || (MAP_TYPE(mtab[c].flag) != MAP_IS_CID && MAP_TYPE(mtab[c].flag) != MAP_IS_CODE)
            || mtab[c - 1].len != mtab[c].len
        {
            break;
        } else if mtab[c - 1].code[..n] == mtab[c].code[..n]
            && mtab[c - 1].code[n] < 255
            && mtab[c - 1].code[n] + 1 == mtab[c].code[n]
        {
            count += 1;
        } else {
            break;
        }
        c += 1;
    }
    count
}

/// `sputx`: two uppercase hex digits at `buf[*s..]` (error past `end`).
fn sputx(c: u8, buf: &mut [u8], s: &mut usize, end: usize) -> Result<i32> {
    let hi = c >> 4;
    let lo = c & 0x0f;
    if *s + 2 > end {
        fatal!("Buffer overflow.");
    }
    buf[*s] = if hi < 10 { hi + b'0' } else { hi + b'7' };
    buf[*s + 1] = if lo < 10 { lo + b'0' } else { lo + b'7' };
    *s += 2;
    Ok(2)
}

/// `write_string` (duplicated from pdfobj.c): a PDF literal string (C
/// stops at a NUL).
fn write_string(buf: &mut [u8], outptr: &mut usize, endptr: usize, string_data: &[u8]) {
    let _ = endptr;
    let mut out = Buf::new();
    out.push(b'(');
    for &ch in string_data.iter().take_while(|&&c| c != 0) {
        if !(32..=126).contains(&ch) {
            let _ = write!(out, "\\{ch:03o}");
        } else {
            match ch {
                b'(' | b')' | b'\\' => {
                    out.push(b'\\');
                    out.push(ch);
                }
                _ => out.push(ch),
            }
        }
    }
    out.push(b')');
    let p = *outptr;
    buf[p..p + out.len()].copy_from_slice(out.as_bytes());
    *outptr += out.len();
}

/// `is_delim` as cmap_write.c defines it (with `{` and `}`).
fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%'
    )
}

/// `write_name`: a PDF name (`#xx` escapes; C stops at a NUL).
fn write_name(buf: &mut [u8], outptr: &mut usize, endptr: usize, name_data: &[u8]) -> Result<()> {
    buf[*outptr] = b'/';
    *outptr += 1;
    for &c in name_data.iter().take_while(|&&c| c != 0) {
        // (C's `char` is signed: bytes from 0x80 are below '!'.)
        if c < b'!' || c > b'~' || c == b'#' || is_delim(c) {
            // "space" is here.
            buf[*outptr] = b'#';
            *outptr += 1;
            sputx(c, buf, outptr, endptr)?;
        } else {
            buf[*outptr] = c;
            *outptr += 1;
        }
    }
    Ok(())
}

impl Dpx {
    /// `pdf_add_stream(stream, fmt_buf)` of `"%d <word>\n"`.
    fn cmap_write_count(&mut self, stream: Obj, n: i32, word: &[u8]) -> Result<()> {
        let mut b = Buf::new();
        b.int(n);
        b.push(b' ');
        b.extend(word);
        b.push(b'\n');
        self.o.add_stream(stream, b.as_bytes())?;
        Ok(())
    }

    /// The bfchar flush: `count beginbfchar`, `wbuf`, `endbfchar`.
    fn cmap_write_flush_bfchar(&mut self, stream: Obj, count: i32, wbuf: &mut Sbuf) -> Result<()> {
        self.cmap_write_count(stream, count, b"beginbfchar")?;
        self.o.add_stream(stream, wbuf.written())?;
        wbuf.curptr = 0;
        self.o.add_stream(stream, b"endbfchar\n")?;
        Ok(())
    }

    /// `write_map`: `mtab` a 256-entry table, `codestr[..depth]` the code
    /// prefix; flushes `wbuf` into `stream` when full.
    fn write_map(
        &mut self,
        mtab: &[MapDef],
        count: i32,
        codestr: &mut [u8],
        depth: i32,
        wbuf: &mut Sbuf,
        stream: Obj,
    ) -> Result<i32> {
        let mut count = count;
        let d = depth as usize;
        // (start, count)
        let mut blocks: Vec<(i32, i32)> = Vec::with_capacity(256 / BLOCK_LEN_MIN as usize + 1);

        let mut c: i32 = 0;
        while c < 256 {
            codestr[d] = (c & 0xff) as u8;
            let e = &mtab[c as usize];
            if LOOKUP_CONTINUE(e.flag) {
                let mtab1 = e.next.as_ref().expect("CMap table without next");
                count = self.write_map(mtab1, count, codestr, depth + 1, wbuf, stream)?;
            } else if MAP_DEFINED(e.flag) {
                match MAP_TYPE(e.flag) {
                    MAP_IS_CID | MAP_IS_CODE => {
                        let block_length = block_count(mtab, c);
                        if block_length >= BLOCK_LEN_MIN {
                            blocks.push((c, block_length));
                            c += block_length;
                        } else {
                            wbuf.put(b'<');
                            for i in 0..=d {
                                wbuf.putx(codestr[i])?;
                            }
                            wbuf.put(b'>');
                            wbuf.put(b' ');
                            wbuf.put(b'<');
                            for i in 0..e.len as usize {
                                wbuf.putx(e.code[i])?;
                            }
                            wbuf.put(b'>');
                            wbuf.put(b'\n');
                            count += 1;
                        }
                    }
                    MAP_IS_NAME => {
                        fatal!("{}: Unexpected error...", CMAP_DEBUG_STR);
                    }
                    MAP_IS_NOTDEF => {}
                    t => {
                        fatal!("{}: Unknown mapping type: {}", CMAP_DEBUG_STR, t);
                    }
                }
            }

            // Flush if necessary
            if count >= 100 || wbuf.curptr >= wbuf.limptr {
                if count > 100 {
                    fatal!("Unexpected error....: {}", count);
                }
                self.cmap_write_flush_bfchar(stream, count, wbuf)?;
                count = 0;
            }
            c += 1;
        }

        if !blocks.is_empty() {
            if count > 0 {
                self.cmap_write_flush_bfchar(stream, count, wbuf)?;
                count = 0;
            }
            self.cmap_write_count(stream, blocks.len() as i32, b"beginbfrange")?;
            for &(start, bcount) in &blocks {
                let c = start;
                wbuf.put(b'<');
                for j in 0..d {
                    wbuf.putx(codestr[j])?;
                }
                wbuf.putx(c as u8)?;
                wbuf.put(b'>');
                wbuf.put(b' ');
                wbuf.put(b'<');
                for j in 0..d {
                    wbuf.putx(codestr[j])?;
                }
                wbuf.putx((c + bcount) as u8)?;
                wbuf.put(b'>');
                wbuf.put(b' ');
                wbuf.put(b'<');
                let e = &mtab[c as usize];
                for j in 0..e.len as usize {
                    wbuf.putx(e.code[j])?;
                }
                wbuf.put(b'>');
                wbuf.put(b'\n');
            }
            self.o.add_stream(stream, wbuf.written())?;
            wbuf.curptr = 0;
            self.o.add_stream(stream, b"endbfrange\n")?;
        }

        Ok(count)
    }

    /// `CMap_create_stream`: none for an invalid CMap. A `use_cmap` is an
    /// error (as in C: "UseCMap found (not supported yet)", the code after
    /// it is unreachable). Pass a CMap not borrowed from `self` (clone it
    /// out of the cache if needed).
    pub fn CMap_create_stream(&mut self, cmap: &CMap) -> Result<Option<Obj>> {
        if !self.CMap_is_valid(cmap)? {
            warn!("Invalid CMap");
            return Ok(None);
        }
        if cmap.type_ == CMAP_TYPE_IDENTITY {
            return Ok(None);
        }

        let stream = self.o.new_stream(STREAM_COMPRESS);
        let stream_dict = self.o.stream_dict(stream)?;

        let csi = match cmap.CMap_get_CIDSysInfo() {
            Some(csi) => csi.clone(),
            None => {
                if cmap.type_ != CMAP_TYPE_TO_UNICODE {
                    crate::cid::CSI_IDENTITY()
                } else {
                    crate::cid::CSI_UNICODE()
                }
            }
        };
        let registry = csi.registry.clone().unwrap_or_default();
        let ordering = csi.ordering.clone().unwrap_or_default();
        let name = cmap.name.clone().unwrap_or_default();

        if cmap.type_ != CMAP_TYPE_TO_UNICODE {
            let csi_dict = self.o.new_dict();
            self.o.put_string(csi_dict, b"Registry", cstr(&registry))?;
            self.o.put_string(csi_dict, b"Ordering", cstr(&ordering))?;
            self.o
                .put_number(csi_dict, b"Supplement", f64::from(csi.supplement))?;
            self.o.put_name(stream_dict, b"Type", b"CMap")?;
            self.o.put_name(stream_dict, b"CMapName", cstr(&name))?;
            self.o.put(stream_dict, b"CIDSystemInfo", csi_dict)?;
            if cmap.wmode != 0 {
                self.o
                    .put_number(stream_dict, b"WMode", f64::from(cmap.wmode))?;
            }
        }

        // TODO: Predefined CMaps need not to be embedded.
        if cmap.use_cmap.is_some() {
            fatal!("UseCMap found (not supported yet)...");
        }

        let max_in = cmap.profile.max_bytes_in;
        let max_out = cmap.profile.max_bytes_out;
        let mut wbuf = Sbuf {
            buf: vec![0u8; WBUF_SIZE],
            curptr: 0,
            limptr: (WBUF_SIZE as isize - 2 * (max_in + max_out) as isize + 16) as usize,
        };
        let mut codestr = vec![0u8; max_in.max(0) as usize];

        // Start CMap
        self.o.add_stream(stream, CMAP_BEGIN)?;

        wbuf.puts(b"/CMapName ");
        let lim = wbuf.limptr;
        write_name(&mut wbuf.buf, &mut wbuf.curptr, lim, &name)?;
        wbuf.puts(b" def\n");
        let mut b = Buf::new();
        let _ = write!(b, "/CMapType {} def\n", cmap.type_);
        wbuf.puts(b.as_bytes());
        if cmap.wmode != 0 && cmap.type_ != CMAP_TYPE_TO_UNICODE {
            let mut b = Buf::new();
            let _ = write!(b, "/WMode {} def\n", cmap.wmode);
            wbuf.puts(b.as_bytes());
        }

        wbuf.puts(b"/CIDSystemInfo <<\n");
        wbuf.puts(b"  /Registry ");
        write_string(&mut wbuf.buf, &mut wbuf.curptr, lim, &registry);
        wbuf.puts(b"\n");
        wbuf.puts(b"  /Ordering ");
        write_string(&mut wbuf.buf, &mut wbuf.curptr, lim, &ordering);
        wbuf.puts(b"\n");
        let mut b = Buf::new();
        let _ = write!(b, "  /Supplement {}\n>> def\n", csi.supplement);
        wbuf.puts(b.as_bytes());
        self.o.add_stream(stream, wbuf.written())?;
        wbuf.curptr = 0;

        // codespacerange
        let mut b = Buf::new();
        let _ = write!(b, "{} begincodespacerange\n", cmap.codespace.len());
        wbuf.puts(b.as_bytes());
        for r in &cmap.codespace {
            wbuf.put(b'<');
            for j in 0..r.dim as usize {
                wbuf.putx(r.code_lo[j])?;
            }
            wbuf.put(b'>');
            wbuf.put(b' ');
            wbuf.put(b'<');
            for j in 0..r.dim as usize {
                wbuf.putx(r.code_hi[j])?;
            }
            wbuf.put(b'>');
            wbuf.put(b'\n');
        }
        self.o.add_stream(stream, wbuf.written())?;
        wbuf.curptr = 0;
        self.o.add_stream(stream, b"endcodespacerange\n")?;

        // CMap body
        if let Some(tbl) = cmap.map_tbl.as_ref() {
            let count = self.write_map(tbl, 0, &mut codestr, 0, &mut wbuf, stream)?; // Top node
            if count > 0 {
                // Flush
                if count > 100 {
                    fatal!("Unexpected error....: {}", count);
                }
                self.cmap_write_count(stream, count, b"beginbfchar")?;
                self.o.add_stream(stream, wbuf.written())?;
                self.o.add_stream(stream, b"endbfchar\n")?;
                wbuf.curptr = 0;
            }
        }
        // End CMap
        self.o.add_stream(stream, CMAP_END)?;

        Ok(Some(stream))
    }
}

/// The bytes before the first NUL (C's `strlen`).
fn cstr(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmap::{CMAP_TYPE_CODE_TO_CID, CMAP_TYPE_TO_UNICODE};
    use crate::io::{Files, Format};
    use alloc::sync::Arc;

    struct NoFiles;
    impl Files for NoFiles {
        fn find(&mut self, _: &[u8], _: Format, _: &[u8]) -> Option<Vec<u8>> {
            None
        }
        fn read(&mut self, _: &[u8]) -> Option<Arc<[u8]>> {
            None
        }
    }

    fn dpx() -> Dpx {
        Dpx::new(Box::new(NoFiles), Box::new(|_, d: &[u8]| d.to_vec()))
    }

    fn tounicode() -> CMap {
        let mut c = CMap::CMap_new();
        c.CMap_set_name(b"Test");
        c.CMap_set_type(CMAP_TYPE_TO_UNICODE);
        c.CMap_add_codespacerange(&[0x00], &[0xff]);
        c
    }

    #[test]
    fn tounicode_stream_text() {
        let mut d = dpx();
        let mut c = tounicode();
        for b in [0x41u8, 0x42, 0x43] {
            c.CMap_add_bfchar(&[b], &[0, b]);
        }
        c.CMap_add_bfchar(&[0x50], &[0x00, 0x50, 0x00, 0x60]);
        c.CMap_add_bfchar(&[0x60], &[0x00, 0x61]);
        let s = d.CMap_create_stream(&c).unwrap().unwrap();
        let text = d.o.stream_data(s).unwrap().to_vec();
        let want: &[u8] = b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
/CMapName /Test def\n/CMapType 2 def\n/CIDSystemInfo <<\n  /Registry (Adobe)\n  /Ordering (UCS)\n  /Supplement 0\n>> def\n\
1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
2 beginbfchar\n<50> <00500060>\n<60> <0061>\nendbfchar\n\
1 beginbfrange\n<41> <43> <0041>\nendbfrange\n\
endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";
        assert_eq!(
            core::str::from_utf8(&text).unwrap(),
            core::str::from_utf8(want).unwrap()
        );
    }

    #[test]
    fn hundred_entry_blocks() {
        let mut d = dpx();
        let mut c = tounicode();
        for b in 0u8..150 {
            let v = u16::from(b) * 2;
            c.CMap_add_bfchar(&[b], &v.to_be_bytes());
        }
        let s = d.CMap_create_stream(&c).unwrap().unwrap();
        let text = String::from(core::str::from_utf8(d.o.stream_data(s).unwrap()).unwrap());
        let a = text.find("100 beginbfchar\n<00> <0000>\n").unwrap();
        let b = text
            .find("endbfchar\n50 beginbfchar\n<64> <00C8>\n")
            .unwrap();
        assert!(a < b);
        assert!(text.contains("<95> <012A>\nendbfchar\nendcmap\n"));
    }

    #[test]
    fn name_and_string_escapes() {
        let mut buf = vec![0u8; 64];
        let mut p = 0;
        write_name(&mut buf, &mut p, 64, b"A B#(\x80").unwrap();
        write_string(&mut buf, &mut p, 64, b"a(b)\\\x01");
        assert_eq!(&buf[..p], b"/A#20B#23#28#80(a\\(b\\)\\\\\\001)");
    }

    #[test]
    fn decode_identity_and_cid() {
        let mut d = dpx();
        d.CMap_cache_init().unwrap();
        let input = [0x00u8, 0x41, 0x01, 0x02];
        let mut out = [0u8; 4];
        let (mut ip, mut il, mut op, mut ol) = (0usize, 4i32, 0usize, 4i32);
        let n = {
            let id = d.CMap_cache_get(0).unwrap();
            d.CMap_decode(id, &input, &mut ip, &mut il, &mut out, &mut op, &mut ol)
                .unwrap()
        };
        assert_eq!((n, ip, il, op, ol), (2, 4, 0, 4, 0));
        assert_eq!(out, input);

        let mut c = CMap::CMap_new();
        c.CMap_set_name(b"T-H");
        c.CMap_set_type(CMAP_TYPE_CODE_TO_CID);
        c.CMap_add_codespacerange(&[0x00], &[0x80]);
        c.CMap_add_codespacerange(&[0x81, 0x40], &[0x9f, 0xfc]);
        c.CMap_add_cidrange(&[0x20], &[0x7e], 1);
        c.CMap_add_cidrange(&[0x81, 0x40], &[0x81, 0x42], 633);
        let input = [0x41u8, 0x81, 0x41, 0x7f];
        let mut out = [0u8; 6];
        let (mut ip, mut il, mut op, mut ol) = (0usize, 4i32, 0usize, 6i32);
        let n = d
            .CMap_decode(&c, &input, &mut ip, &mut il, &mut out, &mut op, &mut ol)
            .unwrap();
        assert_eq!(n, 3);
        assert_eq!(out, [0, 34, 0x02, 0x7a, 0, 0]); // 0x7f undefined: notdef
        assert_eq!((ip, il, op, ol), (4, 0, 6, 0));
    }
}
