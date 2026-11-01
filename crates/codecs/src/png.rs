//! PNG images as pdfTeX includes them (`writepng.c`), read as libpng
//! 1.6.58 reads them with pdfTeX's settings: [`read_info`] is
//! `png_read_info` (the signature, then the chunks before the first
//! IDAT), [`read_image`] the transformations pdfTeX asks for and the rows
//! `png_read_row` (interlaced: `png_read_image`) then returns.
//!
//! libpng's rules decide what counts: the table of `png_handle_chunk`
//! (position, repetition, length) and each chunk's handler say which
//! chunks are valid, and pdfTeX's choice between copying the IDAT stream
//! and writing the pixels again depends on those [`Info::valid`] bits.
//! Whatever libpng ends with `png_error` is an [`Error`] (pdfTeX stops
//! there); its warnings, and the benign errors a read structure turns
//! into warnings, are silent, as pdfTeX's warning function makes them.
//!
//! The image data goes through a port of zlib 1.3.2's `inflate`, fed the
//! IDAT chunks in libpng's 8,192-byte reads, row by row: where zlib stops
//! within a read decides whether a damaged end of the stream is an error
//! (met while reading the last row) or a warning (met after it).
//!
//! A row's bits past its last pixel (a sub-byte depth not filling the
//! last byte) are whatever pdfTeX's row buffer held, which `malloc` does
//! not define: here they are 0, as for a zeroed buffer.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

/// What stops libpng: `png_error`, or `png_chunk_error` naming the chunk
/// being read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    chunk: Option<u32>,
    msg: &'static str,
}

impl Error {
    /// `png_error`.
    fn new(msg: &'static str) -> Error {
        Error { chunk: None, msg }
    }
}

impl fmt::Display for Error {
    /// libpng's message (`png_format_buffer` puts the chunk's type first).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = self.chunk {
            for c in name.to_be_bytes() {
                if c.is_ascii_alphabetic() {
                    write!(f, "{}", char::from(c))?;
                } else {
                    write!(f, "[{c:02X}]")?;
                }
            }
            f.write_str(": ")?;
        }
        f.write_str(self.msg)
    }
}

/// The bits of [`Info::valid`] (`PNG_INFO_*`): a chunk read and accepted.
pub const INFO_GAMA: u32 = 0x0001;
/// sBIT.
pub const INFO_SBIT: u32 = 0x0002;
/// cHRM.
pub const INFO_CHRM: u32 = 0x0004;
/// PLTE.
pub const INFO_PLTE: u32 = 0x0008;
/// tRNS.
pub const INFO_TRNS: u32 = 0x0010;
/// bKGD.
pub const INFO_BKGD: u32 = 0x0020;
/// hIST (never, in libpng 1.6.58: its table wants it before PLTE).
pub const INFO_HIST: u32 = 0x0040;
/// pHYs.
pub const INFO_PHYS: u32 = 0x0080;
/// oFFs.
pub const INFO_OFFS: u32 = 0x0100;
/// tIME.
pub const INFO_TIME: u32 = 0x0200;
/// pCAL.
pub const INFO_PCAL: u32 = 0x0400;
/// sRGB.
pub const INFO_SRGB: u32 = 0x0800;
/// iCCP.
pub const INFO_ICCP: u32 = 0x1000;
/// sPLT.
pub const INFO_SPLT: u32 = 0x2000;
/// sCAL.
pub const INFO_SCAL: u32 = 0x4000;
/// eXIf.
pub const INFO_EXIF: u32 = 0x1_0000;
/// cICP.
pub const INFO_CICP: u32 = 0x2_0000;
/// cLLI.
pub const INFO_CLLI: u32 = 0x4_0000;
/// mDCV.
pub const INFO_MDCV: u32 = 0x8_0000;

/// `PNG_COLOR_TYPE_GRAY`.
pub const COLOR_GRAY: u8 = 0;
/// `PNG_COLOR_TYPE_RGB`.
pub const COLOR_RGB: u8 = 2;
/// `PNG_COLOR_TYPE_PALETTE`.
pub const COLOR_PALETTE: u8 = 3;
/// `PNG_COLOR_TYPE_GRAY_ALPHA`.
pub const COLOR_GRAY_ALPHA: u8 = 4;
/// `PNG_COLOR_TYPE_RGB_ALPHA`.
pub const COLOR_RGB_ALPHA: u8 = 6;
const MASK_COLOR: u8 = 2;
const MASK_ALPHA: u8 = 4;

/// A chunk type as libpng holds it (`PNG_U32`).
const fn chunk_name(n: [u8; 4]) -> u32 {
    u32::from_be_bytes(n)
}

const IHDR: u32 = chunk_name(*b"IHDR");
const PLTE: u32 = chunk_name(*b"PLTE");
const IDAT: u32 = chunk_name(*b"IDAT");
const IEND: u32 = chunk_name(*b"IEND");
const BKGD: u32 = chunk_name(*b"bKGD");
const CHRM: u32 = chunk_name(*b"cHRM");
const CICP: u32 = chunk_name(*b"cICP");
const CLLI: u32 = chunk_name(*b"cLLI");
const EXIF: u32 = chunk_name(*b"eXIf");
const GAMA: u32 = chunk_name(*b"gAMA");
const HIST: u32 = chunk_name(*b"hIST");
const ICCP: u32 = chunk_name(*b"iCCP");
const ITXT: u32 = chunk_name(*b"iTXt");
const MDCV: u32 = chunk_name(*b"mDCV");
const OFFS: u32 = chunk_name(*b"oFFs");
const PCAL: u32 = chunk_name(*b"pCAL");
const PHYS: u32 = chunk_name(*b"pHYs");
const SBIT: u32 = chunk_name(*b"sBIT");
const SCAL: u32 = chunk_name(*b"sCAL");
const SPLT: u32 = chunk_name(*b"sPLT");
const SRGB: u32 = chunk_name(*b"sRGB");
const TEXT: u32 = chunk_name(*b"tEXt");
const TIME: u32 = chunk_name(*b"tIME");
const TRNS: u32 = chunk_name(*b"tRNS");
const ZTXT: u32 = chunk_name(*b"zTXt");

/// `PNG_CHUNK_CRITICAL`: the first letter upper case.
fn critical(name: u32) -> bool {
    name & 0x2000_0000 == 0
}

/// `png_struct::mode` bits.
const HAVE_IHDR: u32 = 0x01;
const HAVE_PLTE: u32 = 0x02;
const HAVE_IDAT: u32 = 0x04;
const AFTER_IDAT: u32 = 0x08;
const HAVE_IEND: u32 = 0x10;

/// `png_chunk_max`: `PNG_USER_CHUNK_MALLOC_MAX`, the largest buffer libpng
/// allocates for a chunk.
const CHUNK_MAX: u32 = 8_000_000;
/// `PNG_USER_WIDTH_MAX`, `PNG_USER_HEIGHT_MAX`.
const USER_MAX: u32 = 1_000_000;
/// `PNG_USER_CHUNK_CACHE_MAX`: how many text and sPLT chunks are read
/// (two less).
const CHUNK_CACHE_MAX: u32 = 1000;
/// `PNG_IDAT_READ_SIZE`.
const IDAT_READ_SIZE: u32 = 8192;
/// `PNG_INFLATE_BUF_SIZE`.
const INFLATE_BUF_SIZE: u32 = 1024;
/// `LZ77Min`: a zlib header, the least deflate data and the checksum.
const LZ77_MIN: u32 = 2 + 5 + 4;

/// zlib's CRC-32 table.
static CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        #[allow(clippy::cast_possible_truncation, reason = "n < 256")]
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
};

/// zlib's `crc32(crc, buf, len)`.
fn crc32(crc: u32, buf: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in buf {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// zlib's `adler32(adler, buf, len)`.
fn adler32(adler: u32, buf: &[u8]) -> u32 {
    let (mut a, mut b) = (adler & 0xffff, adler >> 16);
    for part in buf.chunks(5552) {
        for &c in part {
            a += u32::from(c);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

/// A 32-bit big-endian number (`png_get_uint_32`).
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// A 16-bit big-endian number (`png_get_uint_16`).
fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

// zlib 1.3.2's `inflate`, for a zlib stream with a 32 KiB window, as
// libpng sets it up (`inflateReset2(strm, 15)`): a resumable state
// machine taking what input it is given, stopping where zlib stops.

/// A decoding table entry (zlib's `code`): `op` is 0 for a literal, the
/// index bits of a sub-table (1 to 15), 16 plus the extra bits of a
/// length or distance, 96 for the end of the block, with 64 set for an
/// invalid code; `bits` the code's bits (in this table); `val` the
/// literal, base value or sub-table offset.
#[derive(Clone, Copy, Default)]
struct Code {
    op: u8,
    bits: u8,
    val: u16,
}

/// `codetype`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Codes,
    Lens,
    Dists,
}

/// `ENOUGH_LENS`, `ENOUGH_DISTS`: the largest tables the root sizes 9
/// and 6 need.
const ENOUGH_LENS: u32 = 852;
const ENOUGH_DISTS: u32 = 592;
/// Where the fixed tables start in [`Inflate::codes`], after the dynamic
/// ones (`ENOUGH`).
const FIXED_LEN: usize = 852 + 592;
const FIXED_DIST: usize = FIXED_LEN + 512;

/// The length codes' base values and extra bits (16 + bits; 286 and 287
/// invalid), the distance codes' (30 and 31 invalid).
const LBASE: [u16; 31] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258, 0, 0,
];
const LEXT: [u8; 31] = [
    16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 18, 18, 18, 18, 19, 19, 19, 19, 20, 20, 20, 20,
    21, 21, 21, 21, 16, 199, 75,
];
const DBASE: [u16; 32] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577, 0, 0,
];
const DEXT: [u8; 32] = [
    16, 16, 16, 16, 17, 17, 18, 18, 19, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 24, 25, 25, 26, 26,
    27, 27, 28, 28, 29, 29, 64, 64,
];

/// `inflate_table` (zlib's `inftrees.c`): the decoding table of the code
/// of these lengths, at `table[*next..]`, its root indexed by `*bits`
/// bits (fewer for a shorter code); `next` moves past it. `false` if the
/// lengths over-subscribe the code or leave it incomplete (but for a
/// single one-bit length or distance code). No lengths at all make a
/// table of invalid one-bit codes.
#[allow(
    clippy::cast_possible_truncation,
    reason = "code lengths, symbols and table offsets"
)]
fn inflate_table(
    kind: Kind,
    lens: &[u16],
    table: &mut [Code],
    next: &mut usize,
    bits: &mut u32,
    work: &mut [u16],
) -> bool {
    let mut count = [0u16; 16];
    for &l in lens {
        count[usize::from(l)] += 1;
    }
    let mut max = 15;
    while max >= 1 && count[max] == 0 {
        max -= 1;
    }
    let mut root = (*bits).min(max as u32);
    if max == 0 {
        let here = Code {
            op: 64,
            bits: 1,
            val: 0,
        };
        table[*next] = here;
        table[*next + 1] = here;
        *next += 2;
        *bits = 1;
        return true;
    }
    let mut min = 1;
    while min < max && count[min] == 0 {
        min += 1;
    }
    root = root.max(min as u32);
    let mut left: i32 = 1;
    for &c in &count[1..] {
        left <<= 1;
        left -= i32::from(c);
        if left < 0 {
            return false;
        }
    }
    if left > 0 && (kind == Kind::Codes || max != 1) {
        return false;
    }
    let mut offs = [0u16; 16];
    for len in 1..15 {
        offs[len + 1] = offs[len] + count[len];
    }
    for (sym, &l) in lens.iter().enumerate() {
        if l != 0 {
            let o = &mut offs[usize::from(l)];
            work[usize::from(*o)] = sym as u16;
            *o += 1;
        }
    }
    let (base, extra, first): (&[u16], &[u8], u32) = match kind {
        Kind::Codes => (&[], &[], 20),
        Kind::Lens => (&LBASE, &LEXT, 257),
        Kind::Dists => (&DBASE, &DEXT, 0),
    };
    let start = *next;
    let mut cur = start;
    let mut huff = 0u32;
    let mut sym = 0;
    let mut len = min as u32;
    let mut curr = root;
    let mut drop = 0;
    let mut low = u32::MAX;
    let mut used = 1u32 << root;
    let mask = used - 1;
    let too_big = |used: u32| {
        (kind == Kind::Lens && used > ENOUGH_LENS) || (kind == Kind::Dists && used > ENOUGH_DISTS)
    };
    if too_big(used) {
        return false;
    }
    loop {
        let w = u32::from(work[sym]);
        let b = (len - drop) as u8;
        let here = if w + 1 < first {
            Code {
                op: 0,
                bits: b,
                val: w as u16,
            }
        } else if w >= first {
            Code {
                op: extra[(w - first) as usize],
                bits: b,
                val: base[(w - first) as usize],
            }
        } else {
            Code {
                op: 96,
                bits: b,
                val: 0,
            }
        };
        let incr = 1u32 << (len - drop);
        let mut fill = 1u32 << curr;
        let size = fill;
        loop {
            fill -= incr;
            table[cur + ((huff >> drop) + fill) as usize] = here;
            if fill == 0 {
                break;
            }
        }
        let mut incr = 1u32 << (len - 1);
        while huff & incr != 0 {
            incr >>= 1;
        }
        if incr == 0 {
            huff = 0;
        } else {
            huff &= incr - 1;
            huff += incr;
        }
        sym += 1;
        count[len as usize] -= 1;
        if count[len as usize] == 0 {
            if len == max as u32 {
                break;
            }
            len = u32::from(lens[usize::from(work[sym])]);
        }
        if len > root && huff & mask != low {
            if drop == 0 {
                drop = root;
            }
            cur += size as usize;
            curr = len - drop;
            let mut left = 1i32 << curr;
            while curr + drop < max as u32 {
                left -= i32::from(count[(curr + drop) as usize]);
                if left <= 0 {
                    break;
                }
                curr += 1;
                left <<= 1;
            }
            used += 1 << curr;
            if too_big(used) {
                return false;
            }
            low = huff & mask;
            table[start + low as usize] = Code {
                op: curr as u8,
                bits: root as u8,
                val: (cur - start) as u16,
            };
        }
    }
    if huff != 0 {
        table[cur + huff as usize] = Code {
            op: 64,
            bits: (len - drop) as u8,
            val: 0,
        };
    }
    *next = start + used as usize;
    *bits = root;
    true
}

/// `inflate_mode`, as far as a zlib stream goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Head,
    DictId,
    Dict,
    Type,
    Stored,
    Copy,
    Table,
    LenLens,
    CodeLens,
    Len,
    LenExt,
    Dist,
    DistExt,
    Match,
    Lit,
    Check,
    Done,
    Bad,
}

/// `inflate`'s return codes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ret {
    Ok,
    StreamEnd,
    NeedDict,
    DataError,
    BufError,
}

/// The order of the code length codes' lengths.
const ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// zlib's `inflate_state` (and the `z_stream` around it). Its output
/// goes to `out`, which also serves as the window: it keeps the stream's
/// output from `base` on, at least the last 32 KiB before a call.
struct Inflate {
    mode: Mode,
    last: bool,
    hold: u64,
    bits: u32,
    length: u32,
    offset: u32,
    extra: u32,
    nlen: usize,
    ndist: usize,
    ncode: usize,
    have: usize,
    lens: [u16; 320],
    work: [u16; 288],
    codes: Vec<Code>,
    lencode: usize,
    distcode: usize,
    lenbits: u32,
    distbits: u32,
    /// `strm->msg`.
    msg: Option<&'static str>,
    out: Vec<u8>,
    base: u64,
    /// The Adler-32 of `out[..folded]` (and what was dropped before).
    check: u32,
    folded: usize,
}

impl Inflate {
    /// `inflateReset2(strm, 15)`; the fixed tables built (`inflate_fixed`).
    fn new() -> Inflate {
        let mut z = Inflate {
            mode: Mode::Head,
            last: false,
            hold: 0,
            bits: 0,
            length: 0,
            offset: 0,
            extra: 0,
            nlen: 0,
            ndist: 0,
            ncode: 0,
            have: 0,
            lens: [0; 320],
            work: [0; 288],
            codes: vec![Code::default(); FIXED_DIST + 32],
            lencode: 0,
            distcode: 0,
            lenbits: 0,
            distbits: 0,
            msg: None,
            out: Vec::new(),
            base: 0,
            check: 1,
            folded: 0,
        };
        let mut lens = [8u16; 288];
        lens[144..256].fill(9);
        lens[256..280].fill(7);
        let (mut next, mut bits) = (FIXED_LEN, 9);
        inflate_table(
            Kind::Lens,
            &lens,
            &mut z.codes,
            &mut next,
            &mut bits,
            &mut z.work,
        );
        let mut bits = 5;
        inflate_table(
            Kind::Dists,
            &[5; 32],
            &mut z.codes,
            &mut next,
            &mut bits,
            &mut z.work,
        );
        z
    }

    /// `PULLBYTE`: one more byte of input into the bit buffer, if any.
    fn pull(&mut self, input: &[u8], next: &mut usize) -> bool {
        let Some(&b) = input.get(*next) else {
            return false;
        };
        self.hold |= u64::from(b) << self.bits;
        self.bits += 8;
        *next += 1;
        true
    }

    /// `NEEDBITS`.
    fn need(&mut self, n: u32, input: &[u8], next: &mut usize) -> bool {
        while self.bits < n {
            if !self.pull(input, next) {
                return false;
            }
        }
        true
    }

    /// `BITS`.
    #[allow(clippy::cast_possible_truncation, reason = "at most 16 bits")]
    fn peek(&self, n: u32) -> usize {
        (self.hold & ((1 << n) - 1)) as usize
    }

    /// `DROPBITS`.
    fn dump(&mut self, n: u32) {
        self.hold >>= n;
        self.bits -= n;
    }

    /// `BYTEBITS`.
    fn byte_bits(&mut self) {
        self.dump(self.bits & 7);
    }

    /// `INITBITS`.
    fn init_bits(&mut self) {
        self.hold = 0;
        self.bits = 0;
    }

    /// An error: the message, and the `BAD` mode.
    fn bad(&mut self, msg: &'static str) {
        self.msg = Some(msg);
        self.mode = Mode::Bad;
    }

    /// The table entry of the code at the front of the bit buffer, in the
    /// table at `t` (`root` index bits, and its sub-tables), its bits
    /// dropped; `None` when the input runs out first (the bytes pulled
    /// stay in the bit buffer).
    fn decode(&mut self, t: usize, root: u32, input: &[u8], next: &mut usize) -> Option<Code> {
        let mut here = loop {
            let h = self.codes[t + self.peek(root)];
            if u32::from(h.bits) <= self.bits {
                break h;
            }
            if !self.pull(input, next) {
                return None;
            }
        };
        if here.op != 0 && here.op & 0xf0 == 0 {
            let last = here;
            let lb = u32::from(last.bits);
            here = loop {
                let h = self.codes
                    [t + usize::from(last.val) + (self.peek(lb + u32::from(last.op)) >> lb)];
                if lb + u32::from(h.bits) <= self.bits {
                    break h;
                }
                if !self.pull(input, next) {
                    return None;
                }
            };
            self.dump(lb);
        }
        self.dump(u32::from(here.bits));
        Some(here)
    }

    /// The Adler-32 brought up to the end of the output.
    fn fold(&mut self) {
        self.check = adler32(self.check, &self.out[self.folded..]);
        self.folded = self.out.len();
    }

    /// The output dropped but for the window, once it is large.
    fn trim(&mut self) {
        if self.out.len() > 1 << 20 {
            self.fold();
            let n = self.out.len() - 32768;
            self.out.drain(..n);
            self.base += n as u64;
            self.folded -= n;
        }
    }

    /// `png_zstream_error`: zlib's message, or libpng's for the code.
    fn message(&self, ret: Ret) -> &'static str {
        self.msg.unwrap_or(match ret {
            Ret::Ok => "unexpected zlib return code",
            Ret::StreamEnd => "unexpected end of LZ stream",
            Ret::NeedDict => "missing LZ dictionary",
            Ret::DataError => "damaged LZ stream",
            Ret::BufError => "truncated",
        })
    }

    /// One call of `inflate`: decodes from `input` until it needs more
    /// input, needs to write past `avail_out` more bytes of output, or
    /// the stream ends or is found damaged; the bytes of `input` used
    /// (pulled into the bit buffer or copied) and the return code.
    /// `finish` (`Z_FINISH`) changes only that code.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "bit fields of the bit buffer"
    )]
    fn inflate(&mut self, input: &[u8], avail_out: usize, finish: bool) -> (usize, Ret) {
        let mut next = 0;
        let mut left = avail_out;
        let ret = 'leave: loop {
            match self.mode {
                Mode::Head => {
                    if !self.need(16, input, &mut next) {
                        break 'leave Ret::Ok;
                    }
                    if !(((self.hold & 0xff) << 8) + (self.hold >> 8)).is_multiple_of(31) {
                        self.bad("incorrect header check");
                        continue;
                    }
                    if self.hold & 0xf != 8 {
                        self.bad("unknown compression method");
                        continue;
                    }
                    self.dump(4);
                    if (self.hold & 0xf) + 8 > 15 {
                        self.bad("invalid window size");
                        continue;
                    }
                    self.check = 1;
                    self.mode = if self.hold & 0x200 == 0 {
                        Mode::Type
                    } else {
                        Mode::DictId
                    };
                    self.init_bits();
                }
                Mode::DictId => {
                    if !self.need(32, input, &mut next) {
                        break 'leave Ret::Ok;
                    }
                    self.init_bits();
                    self.mode = Mode::Dict;
                }
                Mode::Dict => return (next, Ret::NeedDict),
                Mode::Type => {
                    if self.last {
                        self.byte_bits();
                        self.mode = Mode::Check;
                        continue;
                    }
                    if !self.need(3, input, &mut next) {
                        break 'leave Ret::Ok;
                    }
                    self.last = self.hold & 1 == 1;
                    self.dump(1);
                    match self.hold & 3 {
                        0 => self.mode = Mode::Stored,
                        1 => {
                            self.lencode = FIXED_LEN;
                            self.lenbits = 9;
                            self.distcode = FIXED_DIST;
                            self.distbits = 5;
                            self.mode = Mode::Len;
                        }
                        2 => self.mode = Mode::Table,
                        _ => self.bad("invalid block type"),
                    }
                    self.dump(2);
                }
                Mode::Stored => {
                    self.byte_bits();
                    if !self.need(32, input, &mut next) {
                        break 'leave Ret::Ok;
                    }
                    if self.hold & 0xffff != ((self.hold >> 16) & 0xffff) ^ 0xffff {
                        self.bad("invalid stored block lengths");
                        continue;
                    }
                    self.length = (self.hold & 0xffff) as u32;
                    self.init_bits();
                    self.mode = Mode::Copy;
                }
                Mode::Copy => {
                    if self.length == 0 {
                        self.mode = Mode::Type;
                        continue;
                    }
                    let n = (self.length as usize).min(input.len() - next).min(left);
                    if n == 0 {
                        break 'leave Ret::Ok;
                    }
                    self.out.extend_from_slice(&input[next..next + n]);
                    next += n;
                    left -= n;
                    self.length -= n as u32;
                }
                Mode::Table => {
                    if !self.need(14, input, &mut next) {
                        break 'leave Ret::Ok;
                    }
                    self.nlen = self.peek(5) + 257;
                    self.dump(5);
                    self.ndist = self.peek(5) + 1;
                    self.dump(5);
                    self.ncode = self.peek(4) + 4;
                    self.dump(4);
                    if self.nlen > 286 || self.ndist > 30 {
                        self.bad("too many length or distance symbols");
                        continue;
                    }
                    self.have = 0;
                    self.mode = Mode::LenLens;
                }
                Mode::LenLens => {
                    while self.have < self.ncode {
                        if !self.need(3, input, &mut next) {
                            break 'leave Ret::Ok;
                        }
                        self.lens[ORDER[self.have]] = (self.hold & 7) as u16;
                        self.have += 1;
                        self.dump(3);
                    }
                    while self.have < 19 {
                        self.lens[ORDER[self.have]] = 0;
                        self.have += 1;
                    }
                    let mut at = 0;
                    self.lencode = 0;
                    self.lenbits = 7;
                    if !inflate_table(
                        Kind::Codes,
                        &self.lens[..19],
                        &mut self.codes,
                        &mut at,
                        &mut self.lenbits,
                        &mut self.work,
                    ) {
                        self.bad("invalid code lengths set");
                        continue;
                    }
                    self.have = 0;
                    self.mode = Mode::CodeLens;
                }
                Mode::CodeLens => {
                    while self.have < self.nlen + self.ndist {
                        let here = loop {
                            let h = self.codes[self.lencode + self.peek(self.lenbits)];
                            if u32::from(h.bits) <= self.bits {
                                break h;
                            }
                            if !self.pull(input, &mut next) {
                                break 'leave Ret::Ok;
                            }
                        };
                        let hb = u32::from(here.bits);
                        if here.val < 16 {
                            self.dump(hb);
                            self.lens[self.have] = here.val;
                            self.have += 1;
                            continue;
                        }
                        let (len, copy) = match here.val {
                            16 => {
                                if !self.need(hb + 2, input, &mut next) {
                                    break 'leave Ret::Ok;
                                }
                                self.dump(hb);
                                if self.have == 0 {
                                    self.bad("invalid bit length repeat");
                                    break;
                                }
                                let copy = 3 + self.peek(2);
                                self.dump(2);
                                (self.lens[self.have - 1], copy)
                            }
                            17 => {
                                if !self.need(hb + 3, input, &mut next) {
                                    break 'leave Ret::Ok;
                                }
                                self.dump(hb);
                                let copy = 3 + self.peek(3);
                                self.dump(3);
                                (0, copy)
                            }
                            _ => {
                                if !self.need(hb + 7, input, &mut next) {
                                    break 'leave Ret::Ok;
                                }
                                self.dump(hb);
                                let copy = 11 + self.peek(7);
                                self.dump(7);
                                (0, copy)
                            }
                        };
                        if self.have + copy > self.nlen + self.ndist {
                            self.bad("invalid bit length repeat");
                            break;
                        }
                        self.lens[self.have..self.have + copy].fill(len);
                        self.have += copy;
                    }
                    if self.mode == Mode::Bad {
                        continue;
                    }
                    if self.lens[256] == 0 {
                        self.bad("invalid code -- missing end-of-block");
                        continue;
                    }
                    let mut at = 0;
                    self.lencode = 0;
                    self.lenbits = 9;
                    if !inflate_table(
                        Kind::Lens,
                        &self.lens[..self.nlen],
                        &mut self.codes,
                        &mut at,
                        &mut self.lenbits,
                        &mut self.work,
                    ) {
                        self.bad("invalid literal/lengths set");
                        continue;
                    }
                    self.distcode = at;
                    self.distbits = 6;
                    if !inflate_table(
                        Kind::Dists,
                        &self.lens[self.nlen..self.nlen + self.ndist],
                        &mut self.codes,
                        &mut at,
                        &mut self.distbits,
                        &mut self.work,
                    ) {
                        self.bad("invalid distances set");
                        continue;
                    }
                    self.mode = Mode::Len;
                }
                Mode::Len => {
                    let Some(here) = self.decode(self.lencode, self.lenbits, input, &mut next)
                    else {
                        break 'leave Ret::Ok;
                    };
                    self.length = u32::from(here.val);
                    if here.op == 0 {
                        self.mode = Mode::Lit;
                    } else if here.op & 32 != 0 {
                        self.mode = Mode::Type;
                    } else if here.op & 64 != 0 {
                        self.bad("invalid literal/length code");
                    } else {
                        self.extra = u32::from(here.op & 15);
                        self.mode = Mode::LenExt;
                    }
                }
                Mode::LenExt => {
                    if self.extra > 0 {
                        if !self.need(self.extra, input, &mut next) {
                            break 'leave Ret::Ok;
                        }
                        self.length += self.peek(self.extra) as u32;
                        self.dump(self.extra);
                    }
                    self.mode = Mode::Dist;
                }
                Mode::Dist => {
                    let Some(here) = self.decode(self.distcode, self.distbits, input, &mut next)
                    else {
                        break 'leave Ret::Ok;
                    };
                    if here.op & 64 != 0 {
                        self.bad("invalid distance code");
                        continue;
                    }
                    self.offset = u32::from(here.val);
                    self.extra = u32::from(here.op & 15);
                    self.mode = Mode::DistExt;
                }
                Mode::DistExt => {
                    if self.extra > 0 {
                        if !self.need(self.extra, input, &mut next) {
                            break 'leave Ret::Ok;
                        }
                        self.offset += self.peek(self.extra) as u32;
                        self.dump(self.extra);
                    }
                    self.mode = Mode::Match;
                }
                Mode::Match => {
                    if left == 0 {
                        break 'leave Ret::Ok;
                    }
                    // (the window holds all output, up to its 32 KiB)
                    if u64::from(self.offset) > self.base + self.out.len() as u64 {
                        self.bad("invalid distance too far back");
                        continue;
                    }
                    let n = (self.length as usize).min(left);
                    let from = self.out.len() - self.offset as usize;
                    for k in from..from + n {
                        let b = self.out[k];
                        self.out.push(b);
                    }
                    left -= n;
                    self.length -= n as u32;
                    if self.length == 0 {
                        self.mode = Mode::Len;
                    }
                }
                Mode::Lit => {
                    if left == 0 {
                        break 'leave Ret::Ok;
                    }
                    self.out.push(self.length as u8);
                    left -= 1;
                    self.mode = Mode::Len;
                }
                Mode::Check => {
                    if !self.need(32, input, &mut next) {
                        break 'leave Ret::Ok;
                    }
                    self.fold();
                    if (self.hold as u32).swap_bytes() != self.check {
                        self.bad("incorrect data check");
                        continue;
                    }
                    self.init_bits();
                    self.mode = Mode::Done;
                }
                Mode::Done => break 'leave Ret::StreamEnd,
                Mode::Bad => break 'leave Ret::DataError,
            }
        };
        if ret == Ret::Ok && (finish || (next == 0 && left == avail_out)) {
            (next, Ret::BufError)
        } else {
            (next, ret)
        }
    }
}

/// The file as libpng reads it (`png_read_data` and the CRC of the chunk
/// being read).
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    crc: u32,
    /// `png_struct::chunk_name`: the last chunk header read.
    name: u32,
}

impl<'a> Reader<'a> {
    /// `png_chunk_error`.
    fn chunk_error(&self, msg: &'static str) -> Error {
        Error {
            chunk: Some(self.name),
            msg,
        }
    }

    /// `png_read_data`: the next `n` bytes, or "Read Error" at the end.
    fn read(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let data = self.data;
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= data.len())
            .ok_or(Error::new("Read Error"))?;
        let s = &data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// `png_crc_read`.
    fn crc_read(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let s = self.read(n)?;
        self.crc = crc32(self.crc, s);
        Ok(s)
    }

    /// `png_crc_finish_critical`: `skip` bytes read, then whether the CRC
    /// is wrong (a warning), or an error for a critical chunk not
    /// `ancillary`.
    fn crc_finish_critical(&mut self, skip: u32, ancillary: bool) -> Result<bool, Error> {
        self.crc_read(skip as usize)?;
        let stored = be32(self.read(4)?);
        if stored == self.crc {
            Ok(false)
        } else if ancillary || !critical(self.name) {
            Ok(true)
        } else {
            Err(self.chunk_error("CRC error"))
        }
    }

    /// `png_crc_finish`.
    fn crc_finish(&mut self, skip: u32) -> Result<bool, Error> {
        self.crc_finish_critical(skip, false)
    }

    /// `png_read_chunk_header`: the next chunk's length; its type becomes
    /// `name`, its CRC starts.
    fn chunk_header(&mut self) -> Result<u32, Error> {
        let buf = self.read(8)?;
        let length = be32(buf);
        if length > 0x7fff_ffff {
            return Err(Error::new("PNG unsigned integer out of range"));
        }
        self.name = be32(&buf[4..]);
        self.crc = crc32(0, &buf[4..]);
        if !check_chunk_name(self.name) {
            return Err(self.chunk_error("bad header (invalid type)"));
        }
        Ok(length)
    }
}

/// `check_chunk_name`: four letters, the third upper case (pngfix's
/// arithmetic).
fn check_chunk_name(name: u32) -> bool {
    let mut name = name & !chunk_name([32, 32, 0, 32]);
    let mut t = (name & !0x1f1f_1f1f) ^ 0x4040_4040;
    name = name.wrapping_sub(chunk_name([65, 65, 65, 65]));
    t |= name;
    name = name.wrapping_sub(chunk_name([25, 25, 25, 26]));
    t |= !name;
    t & 0xe0e0_e0e0 == 0
}

/// `png_get_uint_31`.
fn uint_31(b: &[u8]) -> Result<u32, Error> {
    let v = be32(b);
    if v > 0x7fff_ffff {
        return Err(Error::new("PNG unsigned integer out of range"));
    }
    Ok(v)
}

/// What `png_read_info` leaves of what pdfTeX reads: the header, the
/// palette and transparency, and which chunks were valid.
#[derive(Clone, Debug)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
    pub interlace: u8,
    /// `png_get_PLTE`: empty without a valid PLTE.
    pub palette: Vec<[u8; 3]>,
    /// `png_struct::num_trans`: tRNS entries (1 for gray and RGB).
    pub num_trans: u16,
    /// A palette image's tRNS (its first `num_trans` alphas).
    pub trans_alpha: Vec<u8>,
    /// The transparent gray, red, green, blue (`png_color_16`).
    pub trans_color: [u16; 4],
    /// `info_ptr->valid`: the `INFO_*` bits.
    pub valid: u32,
    /// `png_get_gAMA_fixed`, with [`INFO_GAMA`].
    pub gamma: i32,
    /// pHYs: pixels per unit across and down, the unit (1: meter), with
    /// [`INFO_PHYS`].
    pub phys: Option<(u32, u32, u8)>,
    /// The first IDAT chunk's data: where it starts, its length.
    idat: usize,
    idat_len: u32,
}

impl Info {
    /// `png_get_valid`: the `flag` bits of [`valid`](Info::valid), but
    /// tRNS (asked alone) not once its count is 0.
    #[must_use]
    pub fn get_valid(&self, flag: u32) -> u32 {
        if flag == INFO_TRNS && self.num_trans == 0 {
            return 0;
        }
        self.valid & flag
    }

    /// The image's channels (`png_struct::channels`).
    fn channels(&self) -> u8 {
        match self.color_type {
            COLOR_RGB => 3,
            COLOR_GRAY_ALPHA => 2,
            COLOR_RGB_ALPHA => 4,
            _ => 1,
        }
    }
}

/// A known chunk's rules, the `read_chunks` table of `png_handle_chunk`:
/// its length bounds, the `mode` bits it must come before and after, and
/// whether it may repeat.
struct Rule {
    name: u32,
    max: u32,
    min: u32,
    before: u32,
    after: u32,
    multiple: bool,
}

/// `NoCheck`, `Limit`: no maximum length, or [`CHUNK_MAX`].
const NO_CHECK: u32 = u32::MAX;
const LIMIT: u32 = u32::MAX - 1;
/// `hCOL`: before PLTE and IDAT.
const H_COL: u32 = HAVE_PLTE | HAVE_IDAT;
/// `LKMin`: a keyword, then a zlib stream.
const LK_MIN: u32 = 3 + LZ77_MIN;

const fn rule(name: u32, max: u32, min: u32, before: u32, multiple: bool) -> Rule {
    Rule {
        name,
        max,
        min,
        before,
        after: HAVE_IHDR,
        multiple,
    }
}

/// The known chunks with handlers (IDAT read apart; acTL, fcTL and fdAT
/// have none: unknown).
const RULES: [Rule; 24] = [
    Rule {
        name: IHDR,
        max: 13,
        min: 13,
        before: HAVE_IHDR,
        after: 0,
        multiple: false,
    },
    rule(PLTE, NO_CHECK, 0, 0, true),
    Rule {
        name: IEND,
        max: NO_CHECK,
        min: 0,
        before: 0,
        after: AFTER_IDAT,
        multiple: false,
    },
    rule(BKGD, 6, 1, HAVE_IDAT, false),
    rule(CHRM, 32, 32, H_COL, false),
    rule(CICP, 4, 4, H_COL, false),
    rule(CLLI, 8, 8, H_COL, false),
    rule(EXIF, LIMIT, 4, 0, false),
    rule(GAMA, 4, 4, H_COL, false),
    // (hIST before PLTE: libpng 1.6.58's table)
    rule(HIST, 1024, 0, HAVE_PLTE, false),
    rule(ICCP, NO_CHECK, LK_MIN, H_COL, false),
    rule(ITXT, NO_CHECK, 6, 0, true),
    rule(MDCV, 24, 24, H_COL, false),
    rule(OFFS, 9, 9, HAVE_IDAT, false),
    rule(PCAL, NO_CHECK, 14, HAVE_IDAT, false),
    rule(PHYS, 9, 9, HAVE_IDAT, false),
    rule(SBIT, 4, 1, H_COL, false),
    rule(SCAL, LIMIT, 4, HAVE_IDAT, false),
    rule(SPLT, NO_CHECK, 3, HAVE_IDAT, true),
    rule(SRGB, 1, 1, H_COL, false),
    rule(TEXT, NO_CHECK, 2, 0, true),
    rule(TIME, 7, 7, 0, false),
    rule(TRNS, 256, 0, HAVE_IDAT, false),
    rule(ZTXT, LIMIT, LK_MIN, 0, true),
];

/// The PNG signature.
const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// `png_struct` (and `info_struct`) while `png_read_info` reads.
struct Png<'a> {
    r: Reader<'a>,
    mode: u32,
    /// `png_struct::chunks`: the known chunks handled, by [`RULES`] index.
    seen: u32,
    /// `user_chunk_cache_max`.
    cache_max: u32,
    info: Info,
}

/// `png_read_sig` and `png_read_info`, as pdfTeX's `read_png_info` calls
/// it: the chunks up to the first IDAT, which must come, read and judged
/// by libpng's rules.
pub fn read_info(data: &[u8]) -> Result<Info, Error> {
    let mut p = Png {
        r: Reader {
            data,
            pos: 0,
            crc: 0,
            name: 0,
        },
        mode: 0,
        seen: 0,
        cache_max: CHUNK_CACHE_MAX,
        info: Info {
            width: 0,
            height: 0,
            bit_depth: 0,
            color_type: 0,
            interlace: 0,
            palette: Vec::new(),
            num_trans: 0,
            trans_alpha: Vec::new(),
            trans_color: [0; 4],
            valid: 0,
            gamma: 0,
            phys: None,
            idat: 0,
            idat_len: 0,
        },
    };
    let sig = p.r.read(8)?;
    if sig != SIGNATURE {
        return Err(Error::new(if sig[..4] == SIGNATURE[..4] {
            "PNG file corrupted by ASCII conversion"
        } else {
            "Not a PNG file"
        }));
    }
    loop {
        let length = p.r.chunk_header()?;
        if p.r.name == IDAT {
            if p.mode & HAVE_IHDR == 0 {
                return Err(p.r.chunk_error("Missing IHDR before IDAT"));
            }
            if p.info.color_type == COLOR_PALETTE && p.mode & HAVE_PLTE == 0 {
                return Err(p.r.chunk_error("Missing PLTE before IDAT"));
            }
            p.info.idat = p.r.pos;
            p.info.idat_len = length;
            return Ok(p.info);
        }
        p.handle_chunk(length)?;
    }
}

impl Png<'_> {
    /// `png_handle_chunk`: the table's checks (an error for a critical
    /// chunk, skipped for an ancillary one), then the handler; a chunk
    /// handled is seen.
    fn handle_chunk(&mut self, length: u32) -> Result<(), Error> {
        let name = self.r.name;
        let Some(index) = RULES.iter().position(|r| r.name == name) else {
            return self.handle_unknown(length);
        };
        if name != IHDR && self.mode & HAVE_IHDR == 0 {
            return Err(self.r.chunk_error("missing IHDR"));
        }
        let rule = &RULES[index];
        let errmsg = if self.mode & rule.before != 0 || self.mode & rule.after != rule.after {
            Some("out of place")
        } else if !rule.multiple && self.seen & 1 << index != 0 {
            Some("duplicate")
        } else if length < rule.min {
            Some("too short")
        } else {
            match rule.max {
                NO_CHECK => None,
                LIMIT => (length > CHUNK_MAX).then_some("length exceeds libpng limit"),
                max => (length > max).then_some("too long"),
            }
        };
        if let Some(msg) = errmsg {
            if critical(name) {
                return Err(self.r.chunk_error(msg));
            }
            self.r.crc_finish(length)?;
            return Ok(());
        }
        let handled = match name {
            IHDR => self.handle_ihdr()?,
            PLTE => self.handle_plte(length)?,
            IEND => self.handle_iend(length)?,
            BKGD => self.handle_bkgd(length)?,
            CHRM => self.handle_chrm()?,
            CICP => self.handle_fixed(4, INFO_CICP, |b| b[2] == 0)?,
            CLLI => self.handle_fixed(8, INFO_CLLI, |b| {
                be32(b) <= 0x7fff_ffff && be32(&b[4..]) <= 0x7fff_ffff
            })?,
            EXIF => self.handle_exif(length)?,
            GAMA => self.handle_gama()?,
            HIST => self.handle_hist(length)?,
            ICCP => self.handle_iccp(length)?,
            MDCV => self.handle_fixed(24, INFO_MDCV, |b| {
                be32(&b[16..]) <= 0x7fff_ffff && be32(&b[20..]) <= 0x7fff_ffff
            })?,
            OFFS => self.handle_fixed(9, INFO_OFFS, |_| true)?,
            PCAL => self.handle_pcal(length)?,
            PHYS => self.handle_phys()?,
            SBIT => self.handle_sbit(length)?,
            SCAL => self.handle_scal(length)?,
            SPLT => self.handle_splt(length)?,
            SRGB => self.handle_srgb()?,
            TIME => self.handle_fixed(7, INFO_TIME, |b| {
                (1..=12).contains(&b[2])
                    && (1..=31).contains(&b[3])
                    && b[4] <= 23
                    && b[5] <= 59
                    && b[6] <= 60
            })?,
            TRNS => self.handle_trns(length)?,
            // tEXt, zTXt, iTXt
            _ => self.handle_text(length)?,
        };
        if handled {
            self.seen |= 1 << index;
        }
        Ok(())
    }

    /// `png_handle_unknown`, no unknown chunk kept: skipped, but a
    /// critical one is an error.
    fn handle_unknown(&mut self, length: u32) -> Result<(), Error> {
        self.r.crc_finish(length)?;
        if critical(self.r.name) {
            return Err(self.r.chunk_error("unhandled critical chunk"));
        }
        Ok(())
    }

    /// The handlers of chunks of a fixed length whose data only decides
    /// whether they are valid (`png_handle_cICP`, `cLLI`, `mDCV`, `oFFs`,
    /// `tIME`, through their `png_set_*`): handled even when not valid.
    fn handle_fixed(
        &mut self,
        n: usize,
        bit: u32,
        ok: impl Fn(&[u8]) -> bool,
    ) -> Result<bool, Error> {
        let buf = self.r.crc_read(n)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        if ok(buf) {
            self.info.valid |= bit;
        }
        Ok(true)
    }

    /// `png_handle_IHDR`, `png_set_IHDR` and `png_check_IHDR`.
    fn handle_ihdr(&mut self) -> Result<bool, Error> {
        self.mode |= HAVE_IHDR;
        let buf = self.r.crc_read(13)?;
        self.r.crc_finish(0)?;
        let width = uint_31(buf)?;
        let height = uint_31(&buf[4..])?;
        let (depth, color, compression, filter, interlace) =
            (buf[8], buf[9], buf[10], buf[11], buf[12]);
        let i = &mut self.info;
        i.width = width;
        i.height = height;
        i.bit_depth = depth;
        i.color_type = color;
        i.interlace = interlace;
        if width == 0
            || width > USER_MAX
            || height == 0
            || height > USER_MAX
            || !matches!(depth, 1 | 2 | 4 | 8 | 16)
            || matches!(color, 1 | 5 | 7..)
            || (color == COLOR_PALETTE && depth > 8)
            || (matches!(color, COLOR_RGB | COLOR_GRAY_ALPHA | COLOR_RGB_ALPHA) && depth < 8)
            || interlace >= 2
            || compression != 0
            || filter != 0
        {
            return Err(Error::new("Invalid IHDR data"));
        }
        Ok(true)
    }

    /// `png_handle_PLTE` and `png_set_PLTE`: an error in a palette
    /// image, else ignored (and then even a CRC error is not).
    fn handle_plte(&mut self, length: u32) -> Result<bool, Error> {
        let palette_image = self.info.color_type == COLOR_PALETTE;
        let seen = |name| {
            RULES
                .iter()
                .position(|r| r.name == name)
                .is_some_and(|i| self.seen & 1 << i != 0)
        };
        let errmsg = if self.mode & HAVE_PLTE != 0 {
            Some("duplicate")
        } else if self.mode & HAVE_IDAT != 0 {
            Some("out of place")
        } else if self.info.color_type & MASK_COLOR == 0 {
            Some("ignored in grayscale PNG")
        } else if length > 3 * 256 || !length.is_multiple_of(3) {
            Some("invalid")
        } else if !palette_image && (seen(TRNS) || seen(BKGD)) {
            Some("out of place")
        } else {
            None
        };
        if let Some(msg) = errmsg {
            if palette_image {
                self.r.crc_finish(length)?;
                return Err(self.r.chunk_error(msg));
            }
            self.r.crc_finish_critical(length, true)?;
            return Ok(false);
        }
        let max = if palette_image {
            1u32 << self.info.bit_depth
        } else {
            256
        };
        let num = if length > 3 * max { max } else { length / 3 };
        let buf = self.r.crc_read(3 * num as usize)?;
        self.r
            .crc_finish_critical(length - 3 * num, !palette_image)?;
        self.mode |= HAVE_PLTE;
        if num == 0 {
            return Err(Error::new("Invalid palette"));
        }
        self.info.palette = buf.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
        self.info.valid |= INFO_PLTE;
        Ok(true)
    }

    /// `png_handle_IEND` (never before IDAT: out of place).
    fn handle_iend(&mut self, length: u32) -> Result<bool, Error> {
        self.mode |= AFTER_IDAT | HAVE_IEND;
        self.r.crc_finish_critical(length, true)?;
        Ok(true)
    }

    /// `png_handle_gAMA`.
    fn handle_gama(&mut self) -> Result<bool, Error> {
        let buf = self.r.crc_read(4)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        let Ok(gamma) = i32::try_from(be32(buf)) else {
            return Ok(false);
        };
        self.info.gamma = gamma;
        self.info.valid |= INFO_GAMA;
        Ok(true)
    }

    /// `png_handle_sBIT`.
    fn handle_sbit(&mut self, length: u32) -> Result<bool, Error> {
        let (truelen, sample_depth) = if self.info.color_type == COLOR_PALETTE {
            (3, 8)
        } else {
            (self.info.channels(), self.info.bit_depth)
        };
        if length != u32::from(truelen) {
            self.r.crc_finish(length)?;
            return Ok(false);
        }
        let buf = self.r.crc_read(usize::from(truelen))?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        if buf.iter().any(|&b| b == 0 || b > sample_depth) {
            return Ok(false);
        }
        self.info.valid |= INFO_SBIT;
        Ok(true)
    }

    /// `png_handle_cHRM`: no -2^31.
    fn handle_chrm(&mut self) -> Result<bool, Error> {
        let buf = self.r.crc_read(32)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        if buf.chunks(4).any(|b| be32(b) == 0x8000_0000) {
            return Ok(false);
        }
        self.info.valid |= INFO_CHRM;
        Ok(true)
    }

    /// `png_handle_sRGB`.
    fn handle_srgb(&mut self) -> Result<bool, Error> {
        let intent = self.r.crc_read(1)?[0];
        if self.r.crc_finish(0)? || intent > 3 {
            return Ok(false);
        }
        self.info.valid |= INFO_SRGB;
        Ok(true)
    }

    /// `png_handle_iCCP`: the profile inflated in three steps (its
    /// header, its tag table, the rest), checked after each
    /// (`png_icc_check_length`, `_header`, `_tag_table`). A CRC error
    /// once the profile is read leaves it valid.
    fn handle_iccp(&mut self, length: u32) -> Result<bool, Error> {
        let read_length = length.min(81);
        let start = self.r.pos;
        let keyword = self.r.crc_read(read_length as usize)?;
        let mut length = length - read_length;
        if length < LZ77_MIN {
            self.r.crc_finish(length)?;
            return Ok(false);
        }
        let kw = keyword
            .iter()
            .take(80)
            .position(|&c| c == 0)
            .unwrap_or(keyword.len().min(80));
        if (1..=79).contains(&kw) && kw + 1 < keyword.len() && keyword[kw + 1] == 0 {
            let mut z = Inflate::new();
            let mut input = (start + kw + 2, start + keyword.len());
            if self.inflate_read(&mut z, &mut input, &mut length, 132, false)? == 0 {
                let profile_length = be32(&z.out);
                if icc_check_header(profile_length, &z.out[..132], self.info.color_type) {
                    let tags = 12 * be32(&z.out[128..]) as usize;
                    if self.inflate_read(&mut z, &mut input, &mut length, tags, false)? == 0
                        && icc_check_tag_table(profile_length, &z.out[..132 + tags])
                    {
                        let rest = profile_length as usize - 132 - tags;
                        if self.inflate_read(&mut z, &mut input, &mut length, rest, true)? == 0 {
                            self.r.crc_finish(length)?;
                            self.info.valid |= INFO_ICCP;
                            return Ok(true);
                        }
                    }
                }
            }
        }
        self.r.crc_finish(length)?;
        Ok(false)
    }

    /// `png_inflate_read`: `want` more bytes of the chunk's zlib stream
    /// inflated, its data read (`chunk_bytes` left) in 1,024-byte pieces
    /// as zlib asks for them; how many it did not get.
    fn inflate_read(
        &mut self,
        z: &mut Inflate,
        input: &mut (usize, usize),
        chunk_bytes: &mut u32,
        want: usize,
        finish: bool,
    ) -> Result<usize, Error> {
        let mut out_size = want;
        let mut avail_out = 0;
        loop {
            if input.0 == input.1 {
                let n = (*chunk_bytes).min(INFLATE_BUF_SIZE);
                *chunk_bytes -= n;
                let at = self.r.pos;
                self.r.crc_read(n as usize)?;
                *input = (at, at + n as usize);
            }
            if avail_out == 0 {
                avail_out = out_size;
                out_size = 0;
            }
            let before = z.out.len();
            let data = self.r.data;
            let (used, ret) = z.inflate(
                &data[input.0..input.1],
                avail_out,
                *chunk_bytes == 0 && finish,
            );
            input.0 += used;
            avail_out -= z.out.len() - before;
            if ret != Ret::Ok || (out_size == 0 && avail_out == 0) {
                return Ok(out_size + avail_out);
            }
        }
    }

    /// `png_handle_sPLT`: valid with a name, a depth and some entries.
    fn handle_splt(&mut self, length: u32) -> Result<bool, Error> {
        if !self.cache() || length + 1 > CHUNK_MAX {
            self.r.crc_finish(length)?;
            return Ok(false);
        }
        let buf = self.r.crc_read(length as usize)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        let len = buf.len();
        let entry = buf.iter().position(|&c| c == 0).unwrap_or(len) + 1;
        if len < 2 || entry > len - 2 {
            return Ok(false);
        }
        let size = if buf[entry] == 8 { 6 } else { 10 };
        let data = len - entry - 1;
        if data % size != 0 || data == 0 {
            return Ok(false);
        }
        self.info.valid |= INFO_SPLT;
        Ok(true)
    }

    /// The text chunks' (and sPLT's) chunk cache: `false` once used up.
    fn cache(&mut self) -> bool {
        if self.cache_max != 0 {
            if self.cache_max == 1 {
                return false;
            }
            self.cache_max -= 1;
            if self.cache_max == 1 {
                return false;
            }
        }
        true
    }

    /// `png_handle_tEXt`, `zTXt`, `iTXt`: read (the text is no one's).
    fn handle_text(&mut self, length: u32) -> Result<bool, Error> {
        // (zTXt's buffer has no terminator, and its length is limited)
        if !self.cache() || (self.r.name != ZTXT && length + 1 > CHUNK_MAX) {
            self.r.crc_finish(length)?;
            return Ok(false);
        }
        self.r.crc_read(length as usize)?;
        Ok(!self.r.crc_finish(0)?)
    }

    /// `png_handle_tRNS` and `png_set_tRNS`.
    fn handle_trns(&mut self, length: u32) -> Result<bool, Error> {
        let i = &mut self.info;
        let buf = match i.color_type {
            COLOR_GRAY | COLOR_RGB => {
                let n = if i.color_type == COLOR_GRAY { 2 } else { 6 };
                if length != n {
                    self.r.crc_finish(length)?;
                    return Ok(false);
                }
                let buf = self.r.crc_read(n as usize)?;
                i.num_trans = 1;
                if n == 2 {
                    i.trans_color[0] = be16(buf);
                } else {
                    i.trans_color[1] = be16(buf);
                    i.trans_color[2] = be16(&buf[2..]);
                    i.trans_color[3] = be16(&buf[4..]);
                }
                buf
            }
            COLOR_PALETTE => {
                if self.mode & HAVE_PLTE == 0
                    || length as usize > i.palette.len()
                    || length > 256
                    || length == 0
                {
                    self.r.crc_finish(length)?;
                    return Ok(false);
                }
                let buf = self.r.crc_read(length as usize)?;
                i.num_trans = u16::try_from(length).unwrap_or(0);
                buf
            }
            _ => {
                self.r.crc_finish(length)?;
                return Ok(false);
            }
        };
        if self.r.crc_finish(0)? {
            i.num_trans = 0;
            return Ok(false);
        }
        if i.color_type == COLOR_PALETTE {
            i.trans_alpha = buf.to_vec();
        }
        i.valid |= INFO_TRNS;
        Ok(true)
    }

    /// `png_handle_bKGD`: a palette index or sample in range.
    fn handle_bkgd(&mut self, length: u32) -> Result<bool, Error> {
        let i = &self.info;
        let truelen = if i.color_type == COLOR_PALETTE {
            if self.mode & HAVE_PLTE == 0 {
                self.r.crc_finish(length)?;
                return Ok(false);
            }
            1
        } else if i.color_type & MASK_COLOR != 0 {
            6
        } else {
            2
        };
        if length != truelen {
            self.r.crc_finish(length)?;
            return Ok(false);
        }
        let buf = self.r.crc_read(truelen as usize)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        let ok = if i.color_type == COLOR_PALETTE {
            usize::from(buf[0]) < i.palette.len()
        } else if i.bit_depth > 8 {
            true
        } else if i.color_type & MASK_COLOR == 0 {
            buf[0] == 0 && u32::from(buf[1]) < 1 << i.bit_depth
        } else {
            buf[0] == 0 && buf[2] == 0 && buf[4] == 0
        };
        if !ok {
            return Ok(false);
        }
        self.info.valid |= INFO_BKGD;
        Ok(true)
    }

    /// `png_handle_hIST` and `png_set_hIST`: as many entries as the
    /// palette has, which is none before PLTE, so never valid.
    fn handle_hist(&mut self, length: u32) -> Result<bool, Error> {
        let num = length as usize / 2;
        if !length.is_multiple_of(2) || num != self.info.palette.len() || num > 256 {
            self.r.crc_finish(length)?;
            return Ok(false);
        }
        self.r.crc_read(length as usize)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        if num != 0 {
            self.info.valid |= INFO_HIST;
        }
        Ok(true)
    }

    /// `png_handle_pHYs`.
    fn handle_phys(&mut self) -> Result<bool, Error> {
        let buf = self.r.crc_read(9)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        self.info.phys = Some((be32(buf), be32(&buf[4..]), buf[8]));
        self.info.valid |= INFO_PHYS;
        Ok(true)
    }

    /// `png_handle_eXIf`: TIFF's byte order mark first.
    fn handle_exif(&mut self, length: u32) -> Result<bool, Error> {
        let buf = self.r.crc_read(length as usize)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        if !matches!(be32(buf), 0x4949_2a00 | 0x4d4d_002a) {
            return Ok(false);
        }
        self.info.valid |= INFO_EXIF;
        Ok(true)
    }

    /// `png_handle_pCAL` and `png_set_pCAL`: a purpose, the equation's
    /// parameters (as many as its type takes, each a number), units.
    fn handle_pcal(&mut self, length: u32) -> Result<bool, Error> {
        if length + 1 > CHUNK_MAX {
            self.r.crc_finish(length)?;
            return Ok(false);
        }
        let buf = self.r.crc_read(length as usize)?;
        if self.r.crc_finish(0)? {
            return Ok(false);
        }
        let len = buf.len();
        // (the buffer has a 0 after the data)
        let at = |i: usize| buf.get(i).copied().unwrap_or(0);
        let purpose = buf.iter().position(|&c| c == 0).unwrap_or(len);
        if len - purpose <= 12 {
            return Ok(false);
        }
        let (ty, nparams) = (buf[purpose + 9], buf[purpose + 10]);
        if matches!((ty, nparams), (0, 0 | 1 | 3..) | (1 | 2, 0..=2 | 4..) | (3, 0..=3 | 5..)) {
            return Ok(false);
        }
        let mut q = purpose + 11;
        while at(q) != 0 {
            q += 1;
        }
        if nparams == 0 {
            return Ok(false);
        }
        let mut numbers = true;
        for _ in 0..nparams {
            q += 1;
            let s = q;
            while q <= len && at(q) != 0 {
                q += 1;
            }
            if q > len {
                return Ok(false);
            }
            numbers &= check_fp_string(&buf[s..q]);
        }
        if ty <= 3 && numbers {
            self.info.valid |= INFO_PCAL;
        }
        Ok(true)
    }

    /// `png_handle_sCAL`: a unit, then two positive numbers.
    fn handle_scal(&mut self, length: u32) -> Result<bool, Error> {
        if length + 1 > CHUNK_MAX {
            self.r.crc_finish(length)?;
            return Ok(false);
        }
        let buf = self.r.crc_read(length as usize)?;
        if self.r.crc_finish(0)? || !matches!(buf[0], 1 | 2) {
            return Ok(false);
        }
        let (mut i, mut state) = (1, 0);
        if !check_fp_number(buf, &mut state, &mut i) || i >= buf.len() || buf[i] != 0 {
            return Ok(false);
        }
        i += 1;
        if !fp_positive(state) {
            return Ok(false);
        }
        state = 0;
        if !check_fp_number(buf, &mut state, &mut i) || i != buf.len() || !fp_positive(state) {
            return Ok(false);
        }
        self.info.valid |= INFO_SCAL;
        Ok(true)
    }
}

/// `png_icc_check_length` and `png_icc_check_header`: a profile libpng
/// takes for this color type.
fn icc_check_header(length: u32, profile: &[u8], color_type: u8) -> bool {
    let field = |at: usize| be32(&profile[at..]);
    let tags = field(128);
    (132..=CHUNK_MAX).contains(&length)
        && field(0) == length
        && (profile[8] <= 3 || length.is_multiple_of(4))
        && tags <= 357_913_930
        && u64::from(length) >= 132 + 12 * u64::from(tags)
        && field(64) < 0xffff
        && field(36) == chunk_name(*b"acsp")
        && match field(16) {
            0x5247_4220 => color_type & MASK_COLOR != 0,
            0x4752_4159 => color_type & MASK_COLOR == 0,
            _ => false,
        }
        && !matches!(field(12), 0x6162_7374 | 0x6c69_6e6b)
        && matches!(field(20), 0x5859_5a20 | 0x4c61_6220)
}

/// `png_icc_check_tag_table`: every tag inside the profile.
fn icc_check_tag_table(length: u32, profile: &[u8]) -> bool {
    profile[132..].chunks(12).all(|tag| {
        let (start, len) = (be32(&tag[4..]), be32(&tag[8..]));
        start <= length && len <= length - start
    })
}

/// `png_check_fp_number`'s state bits.
const FP_STATE: u32 = 3;
const FP_SAW_SIGN: u32 = 4;
const FP_SAW_DIGIT: u32 = 8;
const FP_SAW_DOT: u32 = 16;
const FP_SAW_E: u32 = 32;
const FP_SAW_ANY: u32 = 60;
const FP_WAS_VALID: u32 = 64;
const FP_NEGATIVE: u32 = 128;
const FP_NONZERO: u32 = 256;
const FP_STICKY: u32 = 448;

/// `png_check_fp_number`: a floating-point number read from `s[*i..]`
/// as far as it goes; whether it has a digit (in its last part).
fn check_fp_number(s: &[u8], state: &mut u32, i: &mut usize) -> bool {
    let set = |state: &mut u32, v: u32| *state = v | (*state & FP_STICKY);
    while let Some(&c) = s.get(*i) {
        let ty = match c {
            b'+' => FP_SAW_SIGN,
            b'-' => FP_SAW_SIGN + FP_NEGATIVE,
            b'.' => FP_SAW_DOT,
            b'0' => FP_SAW_DIGIT,
            b'1'..=b'9' => FP_SAW_DIGIT + FP_NONZERO,
            b'E' | b'e' => FP_SAW_E,
            _ => break,
        };
        // (integer 0, fraction 1, exponent 2) + what it saw
        match (*state & FP_STATE) + (ty & FP_SAW_ANY) {
            4 => {
                if *state & FP_SAW_ANY != 0 {
                    break;
                }
                *state |= ty;
            }
            16 => {
                if *state & FP_SAW_DOT != 0 {
                    break;
                } else if *state & FP_SAW_DIGIT != 0 {
                    *state |= ty;
                } else {
                    set(state, 1 | ty);
                }
            }
            8 => {
                if *state & FP_SAW_DOT != 0 {
                    set(state, 1 | FP_SAW_DOT);
                }
                *state |= ty | FP_WAS_VALID;
            }
            32 | 33 => {
                if *state & FP_SAW_DIGIT == 0 {
                    break;
                }
                set(state, 2);
            }
            9 => *state |= ty | FP_WAS_VALID,
            6 => {
                if *state & FP_SAW_ANY != 0 {
                    break;
                }
                *state |= FP_SAW_SIGN;
            }
            10 => *state |= FP_SAW_DIGIT | FP_WAS_VALID,
            _ => break,
        }
        *i += 1;
    }
    *state & FP_SAW_DIGIT != 0
}

/// `png_check_fp_string`: all of `s` a number.
fn check_fp_string(s: &[u8]) -> bool {
    let (mut state, mut i) = (0, 0);
    check_fp_number(s, &mut state, &mut i) && i == s.len()
}

/// `PNG_FP_IS_POSITIVE`.
fn fp_positive(state: u32) -> bool {
    state & (FP_SAW_DIGIT | FP_NEGATIVE | FP_NONZERO) == FP_SAW_DIGIT | FP_NONZERO
}

/// The transformations pdfTeX asks for: `png_set_tRNS_to_alpha` (when
/// the image has a valid tRNS), `png_set_strip_alpha` (PDF before 1.4,
/// an alpha channel), `png_set_strip_16` (16 bits, no
/// `\pdfimagehicolor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transforms {
    pub trns_to_alpha: bool,
    pub strip_alpha: bool,
    pub strip_16: bool,
}

/// The rows as libpng returns them, after `png_read_update_info`: the
/// transformed depth and color type, the bytes a row, and the rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub bit_depth: u8,
    pub color_type: u8,
    pub rowbytes: usize,
    /// `height * rowbytes` bytes.
    pub rows: Vec<u8>,
}

/// `PNG_ROWBYTES`.
fn row_bytes(pixel_bits: usize, width: usize) -> usize {
    if pixel_bits >= 8 {
        width * (pixel_bits / 8)
    } else {
        (width * pixel_bits).div_ceil(8)
    }
}

/// The `i`th sample of `depth` (1, 2 or 4) bits, the first the highest.
fn sample(row: &[u8], i: usize, depth: u8) -> u8 {
    let bit = i * usize::from(depth);
    (row[bit / 8] >> (8 - usize::from(depth) - bit % 8)) & ((1 << depth) - 1)
}

/// Adam7's passes (`png_pass_start`, `_inc`, `_ystart`, `_yinc`).
const PASS_START: [usize; 7] = [0, 4, 0, 2, 0, 1, 0];
const PASS_INC: [usize; 7] = [8, 8, 4, 4, 2, 2, 1];
const PASS_YSTART: [usize; 7] = [0, 0, 4, 0, 2, 0, 1];
const PASS_YINC: [usize; 7] = [8, 8, 8, 4, 4, 2, 2];

/// `png_struct::transformations` bits.
const EXPAND: u32 = 0x1000;
const EXPAND_TRNS: u32 = 0x200_0000;
const STRIP_ALPHA: u32 = 0x4_0000;
const CHOP_16: u32 = 0x400;

/// pdfTeX's transformations as libpng sets them up
/// (`png_init_read_transformations`) and applies them to a row
/// (`png_do_read_transformations`): expand, strip the alpha, chop to 8
/// bits.
struct Transform {
    transformations: u32,
    color_type: u8,
    bit_depth: u8,
    num_trans: u16,
    /// `png_struct::palette`, `trans_alpha`: 256 entries (black, opaque
    /// past the chunks').
    palette: [[u8; 3]; 256],
    trans_alpha: [u8; 256],
    trans_color: [u16; 4],
}

impl Transform {
    /// `png_set_tRNS_to_alpha` and the others, then the setup of
    /// `png_init_read_transformations`: stripping the alpha drops the
    /// transparency.
    fn new(info: &Info, t: Transforms) -> Transform {
        let mut palette = [[0; 3]; 256];
        palette[..info.palette.len()].copy_from_slice(&info.palette);
        let mut trans_alpha = [0xff; 256];
        trans_alpha[..info.trans_alpha.len()].copy_from_slice(&info.trans_alpha);
        let mut transformations = 0;
        if t.trns_to_alpha {
            transformations |= EXPAND | EXPAND_TRNS;
        }
        if t.strip_alpha {
            transformations = (transformations | STRIP_ALPHA) & !EXPAND_TRNS;
        }
        if t.strip_16 {
            transformations |= CHOP_16;
        }
        Transform {
            transformations,
            color_type: info.color_type,
            bit_depth: info.bit_depth,
            num_trans: if t.strip_alpha { 0 } else { info.num_trans },
            palette,
            trans_alpha,
            trans_color: info.trans_color,
        }
    }

    /// Whether this transformation is on.
    fn has(&self, bit: u32) -> bool {
        self.transformations & bit != 0
    }

    /// `png_read_transform_info`: the transformed color type, depth and
    /// channels.
    fn output(&self) -> (u8, u8, u8) {
        let (mut color, mut depth) = (self.color_type, self.bit_depth);
        if self.has(EXPAND) {
            if color == COLOR_PALETTE {
                color = if self.num_trans > 0 {
                    COLOR_RGB_ALPHA
                } else {
                    COLOR_RGB
                };
                depth = 8;
            } else {
                if self.num_trans != 0 && self.has(EXPAND_TRNS) {
                    color |= MASK_ALPHA;
                }
                depth = depth.max(8);
            }
        }
        if depth == 16 && self.has(CHOP_16) {
            depth = 8;
        }
        let mut channels = if color & MASK_COLOR != 0 && color != COLOR_PALETTE {
            3
        } else {
            1
        };
        if self.has(STRIP_ALPHA) {
            color &= !MASK_ALPHA;
        }
        if color & MASK_ALPHA != 0 {
            channels += 1;
        }
        (color, depth, channels)
    }

    /// `png_do_read_transformations` on a row of `width` pixels.
    fn apply(&self, row: &[u8], width: usize) -> Vec<u8> {
        let (mut color, mut depth) = (self.color_type, self.bit_depth);
        let mut row = row.to_vec();
        if self.has(EXPAND) {
            if color == COLOR_PALETTE {
                row = self.expand_palette(&row, width);
                color = if self.num_trans > 0 {
                    COLOR_RGB_ALPHA
                } else {
                    COLOR_RGB
                };
                depth = 8;
            } else {
                let trans = self.num_trans != 0 && self.has(EXPAND_TRNS);
                (row, color, depth) = self.expand_samples(row, width, trans);
            }
        }
        if self.has(STRIP_ALPHA) && (color == COLOR_RGB_ALPHA || color == COLOR_GRAY_ALPHA) {
            // `png_do_strip_channel`: the last channel dropped
            let (keep, stride) = match (color, depth) {
                (COLOR_GRAY_ALPHA, 8) => (1, 2),
                (COLOR_GRAY_ALPHA, _) => (2, 4),
                (_, 8) => (3, 4),
                _ => (6, 8),
            };
            row = row
                .chunks(stride)
                .flat_map(|p| &p[..keep])
                .copied()
                .collect();
        }
        if self.has(CHOP_16) && depth == 16 {
            // `png_do_chop`: the high bytes
            row = row.iter().step_by(2).copied().collect();
        }
        row
    }

    /// `png_do_expand_palette`: RGB, with tRNS RGBA.
    fn expand_palette(&self, row: &[u8], width: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(width * 4);
        for i in 0..width {
            let idx = if self.bit_depth == 8 {
                row[i]
            } else {
                sample(row, i, self.bit_depth)
            };
            out.extend_from_slice(&self.palette[usize::from(idx)]);
            if self.num_trans > 0 {
                out.push(if u16::from(idx) >= self.num_trans {
                    0xff
                } else {
                    self.trans_alpha[usize::from(idx)]
                });
            }
        }
        out
    }

    /// `png_do_expand`: gray of fewer bits scaled to 8, then with `trans`
    /// an alpha channel for gray and RGB, 0 where the sample matches the
    /// transparent color (its low bits: `trans_color` masked).
    #[allow(clippy::cast_possible_truncation, reason = "bytes of 16-bit samples")]
    fn expand_samples(&self, row: Vec<u8>, width: usize, trans: bool) -> (Vec<u8>, u8, u8) {
        let t = self.trans_color;
        match self.color_type {
            COLOR_GRAY => {
                let (mut row, mut depth, mut gray) = (row, self.bit_depth, u32::from(t[0]));
                if depth < 8 {
                    let scale = match depth {
                        1 => 0xff,
                        2 => 0x55,
                        _ => 0x11,
                    };
                    gray = (gray & ((1 << depth) - 1)) * u32::from(scale);
                    row = (0..width).map(|i| sample(&row, i, depth) * scale).collect();
                    depth = 8;
                }
                if !trans {
                    return (row, COLOR_GRAY, depth);
                }
                let mut out = Vec::with_capacity(row.len() * 2);
                if depth == 8 {
                    for &v in &row {
                        out.push(v);
                        out.push(if u32::from(v) == gray & 0xff { 0 } else { 0xff });
                    }
                } else {
                    let key = [(gray >> 8) as u8, gray as u8];
                    for v in row.chunks(2) {
                        out.extend_from_slice(v);
                        let a = if v == key { 0 } else { 0xff };
                        out.extend_from_slice(&[a, a]);
                    }
                }
                (out, COLOR_GRAY_ALPHA, depth)
            }
            COLOR_RGB if trans => {
                let mut out = Vec::with_capacity(row.len() / 3 * 4);
                if self.bit_depth == 8 {
                    let key = [t[1] as u8, t[2] as u8, t[3] as u8];
                    for v in row.chunks(3) {
                        out.extend_from_slice(v);
                        out.push(if v == key { 0 } else { 0xff });
                    }
                } else {
                    let mut key = [0; 6];
                    for (k, c) in key.chunks_mut(2).zip(&t[1..]) {
                        k.copy_from_slice(&c.to_be_bytes());
                    }
                    for v in row.chunks(6) {
                        out.extend_from_slice(v);
                        let a = if v == key { 0 } else { 0xff };
                        out.extend_from_slice(&[a, a]);
                    }
                }
                (out, COLOR_RGB_ALPHA, self.bit_depth)
            }
            c => (row, c, self.bit_depth),
        }
    }
}

/// `png_read_filter_row` (filters 1 to 4: Sub, Up, Average, Paeth) on
/// `row` against the row above, `bpp` bytes a pixel (at least 1).
fn unfilter(filter: u8, row: &mut [u8], prev: &[u8], bpp: usize) {
    let n = row.len();
    match filter {
        1 => {
            for i in bpp..n {
                row[i] = row[i].wrapping_add(row[i - bpp]);
            }
        }
        2 => {
            for (r, &p) in row.iter_mut().zip(prev) {
                *r = r.wrapping_add(p);
            }
        }
        3 => {
            for i in 0..n {
                let a = if i >= bpp { u16::from(row[i - bpp]) } else { 0 };
                #[allow(clippy::cast_possible_truncation, reason = "the mean of two bytes")]
                let mean = u16::midpoint(a, u16::from(prev[i])) as u8;
                row[i] = row[i].wrapping_add(mean);
            }
        }
        _ => {
            for i in 0..n {
                let (a, c) = if i >= bpp {
                    (i16::from(row[i - bpp]), i16::from(prev[i - bpp]))
                } else {
                    (0, 0)
                };
                let b = i16::from(prev[i]);
                let (p, pc) = (b - c, a - c);
                let (pa, pb, pc) = (p.abs(), pc.abs(), (p + pc).abs());
                let pred = if pa <= pb && pa <= pc {
                    a
                } else if pb <= pc {
                    b
                } else {
                    c
                };
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a byte"
                )]
                let pred = pred as u8;
                row[i] = row[i].wrapping_add(pred);
            }
        }
    }
}

/// The IDAT stream as `png_read_IDAT_data` reads it.
struct Idat<'a> {
    r: Reader<'a>,
    /// `png_struct::idat_size`: the current IDAT chunk's bytes not read.
    size: u32,
    /// The read buffer's bytes zlib has not taken (`next_in`, `avail_in`).
    next: usize,
    end: usize,
    z: Inflate,
    /// `PNG_FLAG_ZSTREAM_ENDED`.
    ended: bool,
}

impl Idat<'_> {
    /// `png_read_IDAT_data`: `want` more bytes inflated into `z.out` (a
    /// row), or with `None`, after the last row, the stream's end looked
    /// for (problems there only warnings, but not a missing IDAT chunk).
    fn read(&mut self, want: Option<usize>) -> Result<(), Error> {
        let mut avail_out = want.unwrap_or(0);
        loop {
            if self.next == self.end {
                while self.size == 0 {
                    self.r.crc_finish(0)?;
                    self.size = self.r.chunk_header()?;
                    if self.r.name != IDAT {
                        return Err(Error::new("Not enough image data"));
                    }
                }
                let n = self.size.min(IDAT_READ_SIZE);
                let at = self.r.pos;
                self.r.crc_read(n as usize)?;
                self.size -= n;
                (self.next, self.end) = (at, at + n as usize);
            }
            let space = if want.is_some() {
                core::mem::take(&mut avail_out)
            } else {
                self.z.trim();
                INFLATE_BUF_SIZE as usize
            };
            let before = self.z.out.len();
            let data = self.r.data;
            let (used, ret) = self.z.inflate(&data[self.next..self.end], space, false);
            self.next += used;
            let produced = self.z.out.len() - before;
            avail_out += if want.is_some() {
                space - produced
            } else {
                produced
            };
            match ret {
                Ret::StreamEnd => {
                    self.ended = true;
                    break;
                }
                Ret::Ok => {}
                _ if want.is_some() => return Err(self.r.chunk_error(self.z.message(ret))),
                _ => return Ok(()),
            }
            if avail_out == 0 {
                break;
            }
        }
        if avail_out > 0 && want.is_some() {
            return Err(Error::new("Not enough image data"));
        }
        Ok(())
    }

    /// One row of `n` bytes as `png_read_row` reads it: inflated, its
    /// filter undone against the row before in `prev`, which it becomes.
    fn row(&mut self, n: usize, prev: &mut Vec<u8>, bpp: usize) -> Result<(), Error> {
        self.z.trim();
        let start = self.z.out.len();
        self.read(Some(n + 1))?;
        let filter = self.z.out[start];
        let mut row = self.z.out[start + 1..start + 1 + n].to_vec();
        if filter > 0 {
            if filter >= 5 {
                return Err(Error::new("bad adaptive filter value"));
            }
            unfilter(filter, &mut row, prev, bpp);
        }
        *prev = row;
        Ok(())
    }

    /// `png_read_finish_IDAT`, after the last row: the stream's end
    /// looked for, the IDAT chunk it ends in read to its CRC.
    fn finish(&mut self) -> Result<(), Error> {
        if !self.ended {
            self.read(None)?;
        }
        self.r.crc_finish(self.size)?;
        Ok(())
    }
}

/// The colour type and depth of the rows [`read_image`] gives for the
/// transformations `t` (`png_read_update_info`), without reading them.
#[must_use]
pub fn output(info: &Info, t: Transforms) -> (u8, u8) {
    let (color_type, bit_depth, _) = Transform::new(info, t).output();
    (color_type, bit_depth)
}

/// The image as pdfTeX's `write_png` reads it: the transformations `t`
/// set (`png_set_tRNS_to_alpha`, `png_set_strip_alpha`,
/// `png_set_strip_16`), `png_set_interlace_handling`,
/// `png_read_update_info`, then every row by `png_read_row`, or for an
/// interlaced image all of them by `png_read_image`.
pub fn read_image(data: &[u8], info: &Info, t: Transforms) -> Result<Image, Error> {
    let x = Transform::new(info, t);
    let (color_type, bit_depth, channels) = x.output();
    let width = info.width as usize;
    let height = info.height as usize;
    let out_bits = usize::from(channels) * usize::from(bit_depth);
    let rowbytes = row_bytes(out_bits, width);
    let mut rows = vec![0u8; rowbytes * height];
    let in_bits = usize::from(info.channels()) * usize::from(info.bit_depth);
    let bpp = in_bits.div_ceil(8);
    let mut s = Idat {
        r: Reader {
            data,
            pos: info.idat,
            crc: crc32(0, b"IDAT"),
            name: IDAT,
        },
        size: info.idat_len,
        next: 0,
        end: 0,
        z: Inflate::new(),
        ended: false,
    };
    if info.interlace == 0 {
        let n = row_bytes(in_bits, width);
        let mut prev = vec![0; n];
        for dst in rows.chunks_mut(rowbytes) {
            s.row(n, &mut prev, bpp)?;
            let row = x.apply(&prev, width);
            dst.copy_from_slice(&row[..rowbytes]);
            // (`png_combine_row` keeps the bits past the last pixel)
            let tail = out_bits * width % 8;
            if tail != 0 {
                dst[rowbytes - 1] &= !(0xff >> tail);
            }
        }
    } else {
        for pass in 0..7 {
            let (start, inc) = (PASS_START[pass], PASS_INC[pass]);
            if width <= start {
                continue;
            }
            let iwidth = (width - start).div_ceil(inc);
            let n = row_bytes(in_bits, iwidth);
            let mut prev = vec![0; n];
            for y in (PASS_YSTART[pass]..height).step_by(PASS_YINC[pass]) {
                s.row(n, &mut prev, bpp)?;
                let row = x.apply(&prev, iwidth);
                let dst = &mut rows[y * rowbytes..(y + 1) * rowbytes];
                // `png_do_read_interlace` and `png_combine_row`: each
                // pixel to its column
                if out_bits >= 8 {
                    let b = out_bits / 8;
                    for (i, px) in row.chunks(b).take(iwidth).enumerate() {
                        let x0 = (start + i * inc) * b;
                        dst[x0..x0 + b].copy_from_slice(px);
                    }
                } else {
                    #[allow(clippy::cast_possible_truncation, reason = "1, 2 or 4")]
                    let depth = out_bits as u8;
                    for i in 0..iwidth {
                        let bit = (start + i * inc) * out_bits;
                        let shift = 8 - out_bits - bit % 8;
                        dst[bit / 8] |= sample(&row, i, depth) << shift;
                    }
                }
            }
        }
    }
    s.finish()?;
    Ok(Image {
        bit_depth,
        color_type,
        rowbytes,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    /// The whole stream inflated, in one call.
    fn inflate_all(z: &[u8]) -> (Vec<u8>, Ret) {
        let mut st = Inflate::new();
        let (_, ret) = st.inflate(z, usize::MAX, false);
        (st.out, ret)
    }

    /// zlib's own streams (fixed and dynamic blocks), as `inflate.rs`
    /// tests them; then fed a byte at a time and with a byte of output
    /// at a time, the same.
    #[test]
    fn inflates_zlib_streams() {
        let vecs: &[(&str, usize, u32, &str)] = &include!("inflate_vectors.rs");
        for &(name, len, crc, h) in vecs {
            let z = hex(h);
            let (out, ret) = inflate_all(&z);
            assert_eq!(ret, Ret::StreamEnd, "{name}");
            assert_eq!(out.len(), len, "{name}");
            assert_eq!(crc32(0, &out), crc, "{name}");
            let mut st = Inflate::new();
            let mut at = 0;
            loop {
                let (used, ret) = st.inflate(&z[at..(at + 1).min(z.len())], 1, false);
                at += used;
                if ret == Ret::StreamEnd {
                    break;
                }
                assert!(matches!(ret, Ret::Ok | Ret::BufError), "{name}");
            }
            assert_eq!(st.out, out, "{name}");
        }
    }

    #[test]
    fn inflates_stored_blocks_and_checks_adler() {
        let data: Vec<u8> = (0..70_000u32)
            .map(|i| u8::try_from(i * 7 % 251).unwrap())
            .collect();
        let mut z = crate::zlib::stored(&data);
        assert_eq!(inflate_all(&z), (data, Ret::StreamEnd));
        *z.last_mut().unwrap() ^= 1;
        let mut st = Inflate::new();
        assert_eq!(st.inflate(&z, usize::MAX, false).1, Ret::DataError);
        assert_eq!(st.msg, Some("incorrect data check"));
    }

    /// A chunk, with its length and CRC.
    fn chunk(out: &mut Vec<u8>, name: [u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        out.extend_from_slice(&name);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(crc32(0, &name), data).to_be_bytes());
    }

    /// A PNG of these chunks (after IHDR) and filtered rows (one IDAT).
    fn png(
        w: u32,
        h: u32,
        depth: u8,
        color: u8,
        interlace: u8,
        chunks: &[(&[u8; 4], &[u8])],
        raw: &[u8],
    ) -> Vec<u8> {
        let mut out = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color, 0, 0, interlace]);
        chunk(&mut out, *b"IHDR", &ihdr);
        for (name, data) in chunks {
            chunk(&mut out, **name, data);
        }
        chunk(&mut out, *b"IDAT", &crate::zlib::stored(raw));
        chunk(&mut out, *b"IEND", &[]);
        out
    }

    fn read(file: &[u8], t: Transforms) -> Result<Image, Error> {
        read_image(file, &read_info(file)?, t)
    }

    /// Each filter, on a gray row of four pixels.
    #[test]
    fn undoes_each_filter() {
        let raw = [
            0, 10, 20, 30, 40, // None
            1, 1, 2, 3, 4, // Sub
            2, 5, 5, 5, 250, // Up
            3, 1, 1, 1, 1, // Average
            4, 1, 2, 3, 4, // Paeth
        ];
        let img = read(
            &png(4, 5, 8, COLOR_GRAY, 0, &[], &raw),
            Transforms::default(),
        )
        .unwrap();
        assert_eq!(
            img.rows,
            [
                10, 20, 30, 40, 1, 3, 6, 10, 6, 8, 11, 4, 4, 7, 10, 8, 5, 9, 13, 14
            ]
        );
    }

    /// Paeth with three bytes a pixel takes its neighbors three bytes
    /// back.
    #[test]
    fn paeth_on_rgb() {
        let raw = [0, 10, 20, 30, 40, 50, 60, 4, 1, 1, 1, 2, 2, 2];
        let img = read(
            &png(2, 2, 8, COLOR_RGB, 0, &[], &raw),
            Transforms::default(),
        )
        .unwrap();
        // row 1: (11, 21, 31) above; then a = (11, 21, 31), b = (40, 50,
        // 60), c = (10, 20, 30): pb = |a - c| = 1 smallest, so b
        assert_eq!(img.rows[6..], [11, 21, 31, 42, 52, 62]);
    }

    /// Adam7: an 8x8 image whose pixel (x, y) is 8y + x, its passes'
    /// rows in order.
    #[test]
    fn deinterlaces_adam7() {
        let mut raw = Vec::new();
        for pass in 0..7 {
            for y in (PASS_YSTART[pass]..8).step_by(PASS_YINC[pass]) {
                raw.push(0);
                for x in (PASS_START[pass]..8).step_by(PASS_INC[pass]) {
                    raw.push(u8::try_from(8 * y + x).unwrap());
                }
            }
        }
        let img = read(
            &png(8, 8, 8, COLOR_GRAY, 1, &[], &raw),
            Transforms::default(),
        )
        .unwrap();
        assert_eq!(img.rows, (0..64).collect::<Vec<u8>>());
    }

    /// A palette with a shorter tRNS: RGBA, opaque past it.
    #[test]
    fn expands_palette_with_trns() {
        let plte = [1, 2, 3, 4, 5, 6, 7, 8, 9];
        let file = png(
            4,
            1,
            2,
            COLOR_PALETTE,
            0,
            &[(b"PLTE", &plte), (b"tRNS", &[0x80])],
            &[0, 0b0001_1011],
        );
        let info = read_info(&file).unwrap();
        assert_eq!(info.get_valid(INFO_TRNS), INFO_TRNS);
        let t = Transforms {
            trns_to_alpha: true,
            ..Transforms::default()
        };
        let img = read_image(&file, &info, t).unwrap();
        assert_eq!(
            (img.color_type, img.bit_depth, img.rowbytes),
            (COLOR_RGB_ALPHA, 8, 16)
        );
        // index 3 is past the palette: black
        assert_eq!(
            img.rows,
            [1, 2, 3, 0x80, 4, 5, 6, 255, 7, 8, 9, 255, 0, 0, 0, 255]
        );
    }

    /// Two-bit gray with a transparent gray: scaled to 8 bits, alpha 0
    /// where it matches.
    #[test]
    fn expands_gray_with_trns() {
        let file = png(
            4,
            1,
            2,
            COLOR_GRAY,
            0,
            &[(b"tRNS", &[0, 2])],
            &[0, 0b0001_1011],
        );
        let t = Transforms {
            trns_to_alpha: true,
            ..Transforms::default()
        };
        let img = read(&file, t).unwrap();
        assert_eq!(img.color_type, COLOR_GRAY_ALPHA);
        assert_eq!(img.rows, [0, 255, 0x55, 255, 0xaa, 0, 255, 255]);
    }

    /// 16-bit RGBA, alpha stripped and chopped to 8 bits.
    #[test]
    fn strips_alpha_and_16_bits() {
        let raw = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        let file = png(1, 1, 16, COLOR_RGB_ALPHA, 0, &[], &raw);
        let t = Transforms {
            strip_alpha: true,
            strip_16: true,
            ..Transforms::default()
        };
        let img = read(&file, t).unwrap();
        assert_eq!(
            (img.color_type, img.bit_depth, img.rows.clone()),
            (COLOR_RGB, 8, vec![1, 3, 5])
        );
    }

    /// One-bit gray three pixels wide: the bits past them are 0.
    #[test]
    fn clears_the_bits_past_the_row() {
        let img = read(
            &png(3, 1, 1, COLOR_GRAY, 0, &[], &[0, 0xff]),
            Transforms::default(),
        );
        assert_eq!(img.unwrap().rows, [0xe0]);
    }

    /// A CRC error is fatal in IHDR, a warning in gAMA (which then does
    /// not count).
    #[test]
    fn crc_errors() {
        let mut file = png(
            1,
            1,
            8,
            COLOR_GRAY,
            0,
            &[(b"gAMA", &100_000u32.to_be_bytes())],
            &[0, 0],
        );
        let info = read_info(&file).unwrap();
        assert_eq!((info.valid, info.gamma), (INFO_GAMA, 100_000));
        // gAMA's CRC: after the signature, IHDR (25), gAMA's 12
        file[8 + 25 + 12] ^= 1;
        assert_eq!(read_info(&file).unwrap().valid, 0);
        file[8 + 8 + 13] ^= 1;
        assert_eq!(read_info(&file).unwrap_err().to_string(), "IHDR: CRC error");
    }

    /// Not enough data, a bad filter, a truncated file.
    #[test]
    fn image_data_errors() {
        let short = png(2, 2, 8, COLOR_GRAY, 0, &[], &[0, 1, 2, 0]);
        assert_eq!(
            read(&short, Transforms::default()).unwrap_err().to_string(),
            "Not enough image data"
        );
        let filter = png(1, 1, 8, COLOR_GRAY, 0, &[], &[5, 0]);
        assert_eq!(
            read(&filter, Transforms::default())
                .unwrap_err()
                .to_string(),
            "bad adaptive filter value"
        );
        let file = png(1, 1, 8, COLOR_GRAY, 0, &[], &[0, 0]);
        let cut = &file[..file.len() - 14];
        assert_eq!(
            read(cut, Transforms::default()).unwrap_err().to_string(),
            "Read Error"
        );
        assert_eq!(
            read_info(&file[..30]).unwrap_err().to_string(),
            "Read Error"
        );
    }
}
