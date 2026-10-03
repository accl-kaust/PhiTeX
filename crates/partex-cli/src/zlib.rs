//! PDF stream compression with the system zlib, the library pdfTeX links:
//! its bytes are pdfTeX's by construction. Called as pdfTeX's writezip.c
//! calls it: one stream at a time, input in pieces of pdfTeX's buffer
//! size, output through a 32 KiB buffer.
#![expect(unsafe_code, reason = "FFI to the system zlib")]

use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};

/// zlib.h's `z_stream` (the ABI is stable across zlib 1.x).
#[repr(C)]
struct ZStream {
    next_in: *const u8,
    avail_in: c_uint,
    total_in: c_ulong,
    next_out: *mut u8,
    avail_out: c_uint,
    total_out: c_ulong,
    msg: *const c_char,
    state: *mut c_void,
    zalloc: *const c_void,
    zfree: *const c_void,
    opaque: *mut c_void,
    data_type: c_int,
    adler: c_ulong,
    reserved: c_ulong,
}

#[link(name = "z")]
unsafe extern "C" {
    fn zlibVersion() -> *const c_char;
    fn deflateInit_(strm: *mut ZStream, level: c_int, version: *const c_char, size: c_int)
    -> c_int;
    fn deflate(strm: *mut ZStream, flush: c_int) -> c_int;
    fn deflateEnd(strm: *mut ZStream) -> c_int;
    fn deflateCopy(dest: *mut ZStream, source: *mut ZStream) -> c_int;
}

const Z_NO_FLUSH: c_int = 0;
const Z_FINISH: c_int = 4;
const Z_OK: c_int = 0;
const Z_STREAM_END: c_int = 1;
/// writezip.c's `ZIP_BUF_SIZE`.
const ZIP_BUF_SIZE: usize = 32768;
/// pdfTeX's `pdf_op_buf_size`: the pieces the stream arrives in.
const PIECE: usize = 16384;
/// The pieces a stream is compressed in when its states are kept, one
/// after each but the last (zlib's output is the same however its input
/// is split: `output_is_the_same_however_the_input_is_split`).
const SNAP: usize = 4096;
/// How many streams [`deflate_stream`] keeps, with its states partway
/// through each, when it resumes ([`resume_streams`]): a rebuild's page,
/// object stream and cross-reference stream, and the last rebuild's.
const KEPT: usize = 6;

/// Whether [`deflate_stream`] resumes from the streams it compressed last.
static RESUME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Let [`deflate_stream`] resume (a session that compresses the same
/// streams again, an edit's page and the cross-reference stream each
/// time with their bytes as before up to the edit's place).
pub fn resume_streams(on: bool) {
    RESUME.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// A zeroed `z_stream`, boxed: zlib's state points back at it, so it
/// must not move once initialized.
#[allow(
    clippy::unnecessary_box_returns,
    reason = "zlib's state points back at the z_stream: it must not move"
)]
fn zstream() -> Box<ZStream> {
    Box::new(ZStream {
        next_in: std::ptr::null(),
        avail_in: 0,
        total_in: 0,
        next_out: std::ptr::null_mut(),
        avail_out: 0,
        total_out: 0,
        msg: std::ptr::null(),
        state: std::ptr::null_mut(),
        zalloc: std::ptr::null(),
        zfree: std::ptr::null(),
        opaque: std::ptr::null_mut(),
        data_type: 0,
        adler: 0,
        reserved: 0,
    })
}

/// A compressor initialized at `level`, or `None` if zlib fails.
fn compressor(level: i32) -> Option<Box<ZStream>> {
    let mut s = zstream();
    let size = c_int::try_from(std::mem::size_of::<ZStream>()).ok()?;
    // SAFETY: `s` is a zeroed z_stream (null allocators select zlib's
    // own), as deflateInit requires.
    if unsafe { deflateInit_(&raw mut *s, level, zlibVersion(), size) } != Z_OK {
        return None;
    }
    Some(s)
}

/// A compressor's state partway through a stream, after a piece that
/// was not the last: zlib's own (a copy), the bytes put out (`out` of
/// them handed on, `buf` still in the output buffer), and the input
/// taken (`at` bytes, a whole number of [`SNAP`]s).
struct Snap {
    z: Box<ZStream>,
    at: usize,
    out: usize,
    buf: Vec<u8>,
}

impl Drop for Snap {
    fn drop(&mut self) {
        // SAFETY: `z` was made by deflateCopy and never moved (boxed).
        unsafe { deflateEnd(&raw mut *self.z) };
    }
}

/// A stream compressed: its input, its output, and the states taken
/// after each of its pieces but the last.
struct Done {
    level: i32,
    data: Vec<u8>,
    out: Vec<u8>,
    snaps: Vec<std::rc::Rc<Snap>>,
}

thread_local! {
    /// The streams [`deflate_stream`] compressed last, the latest first.
    static DONE: std::cell::RefCell<std::collections::VecDeque<Done>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// zlib's compression of `data` at `level`, or `None` if zlib fails.
///
/// Resuming ([`resume_streams`]): a stream compressed before, its bytes
/// the same, is its output again; one whose first bytes are a stream's
/// compressed before goes on from the state that stream's compression
/// was in after the last [`SNAP`] of them (`deflateCopy`), with the
/// output buffer as it was, so its bytes are a compression's from the
/// start.
pub fn deflate_stream(level: i32, data: &[u8]) -> Option<Vec<u8>> {
    if !RESUME.load(std::sync::atomic::Ordering::Relaxed) {
        return compress(level, data, None, PIECE, false).map(|d| d.out);
    }
    DONE.with(|done| {
        let mut done = done.borrow_mut();
        // (the stream with the longest first bytes in common)
        let common = |d: &Done| common_prefix(&d.data, data);
        let best = done
            .iter()
            .enumerate()
            .filter(|(_, d)| d.level == level)
            .map(|(i, d)| (i, common(d)))
            .max_by_key(|&(_, n)| n);
        if let Some((i, n)) = best
            && n == data.len()
            && done[i].data.len() == data.len()
        {
            let d = done.remove(i)?;
            let out = d.out.clone();
            done.push_front(d);
            return Some(out);
        }
        // (the last state whose input is a prefix of `data`, with more of
        // it to come)
        let from = best.and_then(|(i, n)| {
            done[i]
                .snaps
                .iter()
                .rposition(|s| s.at <= n && s.at < data.len())
                .map(|k| (i, k))
        });
        let resumed = from.map(|(i, k)| {
            let d = &done[i];
            (d.snaps[..=k].to_vec(), d.out[..d.snaps[k].out].to_vec())
        });
        let made = compress(level, data, resumed, SNAP, true)?;
        let out = made.out.clone();
        done.push_front(made);
        done.truncate(KEPT);
        Some(out)
    })
}

/// How many first bytes `a` and `b` have in common (compared 64 at a
/// time, then one by one).
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + 64 <= n && a[i..i + 64] == b[i..i + 64] {
        i += 64;
    }
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

/// Compress `data` at `level` as writezip.c does (its output buffer), in
/// pieces of `size` bytes (writezip.c's: [`PIECE`]): from the start, or
/// from the last of `from`'s states with its output so far; with
/// `snaps`, a state kept after each piece but the last (pieces of
/// [`SNAP`] bytes, from a state at a multiple of it).
fn compress(
    level: i32,
    data: &[u8],
    from: Option<(Vec<std::rc::Rc<Snap>>, Vec<u8>)>,
    size: usize,
    snaps: bool,
) -> Option<Done> {
    let (mut s, mut kept, mut out, mut buf, start) = if let Some((kept, out)) = from {
        let last = kept.last()?;
        let mut s = zstream();
        // SAFETY: `last.z` is a live compressor's state, read only; `s` is
        // a zeroed z_stream that deflateCopy initializes.
        if unsafe { deflateCopy(&raw mut *s, (&raw const *last.z).cast_mut()) } != Z_OK {
            return None;
        }
        let mut buf = vec![0u8; ZIP_BUF_SIZE];
        buf[..last.buf.len()].copy_from_slice(&last.buf);
        // SAFETY: the copy's output pointer is set to this buffer, past the
        // bytes it holds.
        s.next_out = unsafe { buf.as_mut_ptr().add(last.buf.len()) };
        s.avail_out = c_uint::try_from(ZIP_BUF_SIZE - last.buf.len()).ok()?;
        let at = last.at;
        (s, kept, out, buf, at)
    } else {
        let mut s = compressor(level)?;
        let mut buf = vec![0u8; ZIP_BUF_SIZE];
        s.next_out = buf.as_mut_ptr();
        s.avail_out = c_uint::try_from(ZIP_BUF_SIZE).ok()?;
        (s, Vec::new(), Vec::new(), buf, 0)
    };
    let rest = &data[start..];
    let pieces: Vec<&[u8]> = if rest.is_empty() {
        vec![&[]]
    } else {
        rest.chunks(size).collect()
    };
    let last = pieces.len() - 1;
    let mut ok = true;
    'pieces: for (i, piece) in pieces.into_iter().enumerate() {
        let finish = i == last;
        s.next_in = piece.as_ptr();
        s.avail_in = c_uint::try_from(piece.len()).ok()?;
        loop {
            if s.avail_out == 0 {
                out.extend_from_slice(&buf);
                s.next_out = buf.as_mut_ptr();
                s.avail_out = c_uint::try_from(ZIP_BUF_SIZE).ok()?;
            }
            // SAFETY: `next_in`/`avail_in` describe `piece` and
            // `next_out`/`avail_out` the rest of `buf`, both alive here.
            let err = unsafe { deflate(&raw mut *s, if finish { Z_FINISH } else { Z_NO_FLUSH }) };
            if finish && err == Z_STREAM_END {
                break 'pieces;
            }
            if err != Z_OK {
                ok = false;
                break 'pieces;
            }
            if !finish && s.avail_in == 0 {
                break;
            }
        }
        if snaps {
            let mut z = zstream();
            // SAFETY: `s` is a live compressor; `z` a zeroed z_stream
            // that deflateCopy initializes (and the Snap's drop ends).
            if unsafe { deflateCopy(&raw mut *z, &raw mut *s) } == Z_OK {
                let used = ZIP_BUF_SIZE - usize::try_from(s.avail_out).unwrap_or(0);
                kept.push(std::rc::Rc::new(Snap {
                    z,
                    at: start + (i + 1) * size,
                    out: out.len(),
                    buf: buf[..used].to_vec(),
                }));
            }
        }
    }
    let used = ZIP_BUF_SIZE - usize::try_from(s.avail_out).unwrap_or(0);
    out.extend_from_slice(&buf[..used]);
    // SAFETY: `s` was initialized by deflateInit_ or deflateCopy.
    unsafe { deflateEnd(&raw mut *s) };
    ok.then(|| Done {
        level,
        data: data.to_vec(),
        out,
        snaps: kept,
    })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::cast_possible_truncation,
        clippy::unreadable_literal,
        reason = "test data"
    )]
    use super::{PIECE, compress, deflate_stream, resume_streams};

    /// Bytes like a page's stream: operators and numbers, some repeated.
    fn page(seed: u64, n: usize) -> Vec<u8> {
        let mut x = seed;
        let mut v = Vec::new();
        while v.len() < n {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let w = (x >> 33) % 7;
            v.extend_from_slice(
                format!(
                    "{}.{} {} Td [({})]TJ\n",
                    w * 13,
                    x % 1000,
                    w,
                    "word".repeat(1 + (x as usize >> 60))
                )
                .as_bytes(),
            );
        }
        v.truncate(n);
        v
    }

    /// zlib's output, its input given in pieces of `piece` bytes.
    fn pieces_of(level: i32, data: &[u8], piece: usize) -> Vec<u8> {
        compress(level, data, None, piece, false).unwrap().out
    }

    /// Whether zlib's output depends on how its input is split.
    #[test]
    fn output_is_the_same_however_the_input_is_split() {
        let mut x = 7u64;
        let mut rand = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|_| {
                    x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    (x >> 56) as u8
                })
                .collect()
        };
        // (an xref stream's entries: a type, three bytes of offset, a generation)
        let mut xref = Vec::new();
        let mut off = 15u32;
        for k in 0..6000u32 {
            if k % 7 == 3 {
                xref.extend_from_slice(&[2, 0, (k / 100) as u8, (k % 100) as u8, 0]);
            } else {
                off += 100 + (k * 37) % 900;
                xref.extend_from_slice(&[1, (off >> 16) as u8, (off >> 8) as u8, off as u8, 0]);
            }
        }
        let datas = [page(3, 90_000), xref, rand(40_000), vec![b'a'; 70_000], {
            let mut v = page(4, 30_000);
            v.extend(rand(5_000));
            v.extend(vec![0u8; 20_000]);
            v.extend(page(5, 30_000));
            v
        }];
        for (j, data) in datas.iter().enumerate() {
            for level in [1, 2, 4, 6, 9] {
                let whole = pieces_of(level, data, 16_384);
                for piece in [1usize, 7, 100, 262, 1000, 2048, 4096, 5000, 65_536] {
                    assert!(
                        pieces_of(level, data, piece) == whole,
                        "data {j}, level {level}, pieces of {piece}"
                    );
                }
            }
        }
    }

    #[test]
    fn resumed_streams_are_compressed_as_from_the_start() {
        resume_streams(true);
        for level in [1, 6, 9] {
            let base = page(1, 70_000);
            assert_eq!(
                deflate_stream(level, &base),
                compress(level, &base, None, PIECE, false).map(|d| d.out)
            );
            // (edits early, mid-piece, at a piece's end, late; a stream as
            // long as a whole number of pieces; shorter, longer)
            for at in [10usize, 16_384, 20_000, 32_768, 40_000, 69_990] {
                for (cut, put) in [(0usize, &b"x"[..]), (3, &b""[..]), (1, &b"longer"[..])] {
                    let mut d = base.clone();
                    d.splice(at..(at + cut).min(d.len()), put.iter().copied());
                    let fresh = compress(level, &d, None, PIECE, false).map(|d| d.out);
                    assert_eq!(deflate_stream(level, &d), fresh, "level {level}, at {at}");
                    // (and the stream before, again)
                    assert_eq!(
                        deflate_stream(level, &base),
                        compress(level, &base, None, PIECE, false).map(|d| d.out)
                    );
                }
            }
            for n in [16_384usize, 32_768, 32_769, 49_152] {
                let d = page(2, n);
                let fresh = compress(level, &d, None, PIECE, false).map(|d| d.out);
                assert_eq!(deflate_stream(level, &d), fresh);
                let mut e = d.clone();
                e.truncate(n - 1);
                assert_eq!(
                    deflate_stream(level, &e),
                    compress(level, &e, None, PIECE, false).map(|d| d.out)
                );
            }
        }
        resume_streams(false);
    }

    #[test]
    fn compresses_to_a_zlib_stream() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let z = deflate_stream(9, &data).expect("zlib");
        assert_eq!(z[0], 0x78);
        assert!(z.len() < data.len() / 10);
        assert_eq!(deflate_stream(6, b"").map(|z| z.len()), Some(8));
    }
}
