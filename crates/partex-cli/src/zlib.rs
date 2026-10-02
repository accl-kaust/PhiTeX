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
}

const Z_NO_FLUSH: c_int = 0;
const Z_FINISH: c_int = 4;
const Z_OK: c_int = 0;
const Z_STREAM_END: c_int = 1;
/// writezip.c's `ZIP_BUF_SIZE`.
const ZIP_BUF_SIZE: usize = 32768;
/// pdfTeX's `pdf_op_buf_size`: the pieces the stream arrives in.
const PIECE: usize = 16384;

/// zlib's compression of `data` at `level`, or `None` if zlib fails.
pub fn deflate_stream(level: i32, data: &[u8]) -> Option<Vec<u8>> {
    let mut s = ZStream {
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
    };
    let size = c_int::try_from(std::mem::size_of::<ZStream>()).ok()?;
    // SAFETY: `s` is a zeroed z_stream (null allocators select zlib's
    // own), as deflateInit requires.
    if unsafe { deflateInit_(&raw mut s, level, zlibVersion(), size) } != Z_OK {
        return None;
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; ZIP_BUF_SIZE];
    let pieces: Vec<&[u8]> = if data.is_empty() {
        vec![&[]]
    } else {
        data.chunks(PIECE).collect()
    };
    let last = pieces.len() - 1;
    let mut ok = true;
    s.next_out = buf.as_mut_ptr();
    s.avail_out = c_uint::try_from(ZIP_BUF_SIZE).ok()?;
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
            let err = unsafe { deflate(&raw mut s, if finish { Z_FINISH } else { Z_NO_FLUSH }) };
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
    }
    let used = ZIP_BUF_SIZE - usize::try_from(s.avail_out).unwrap_or(0);
    out.extend_from_slice(&buf[..used]);
    // SAFETY: `s` was initialized by deflateInit_.
    unsafe { deflateEnd(&raw mut s) };
    ok.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::deflate_stream;

    #[test]
    fn compresses_to_a_zlib_stream() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let z = deflate_stream(9, &data).expect("zlib");
        assert_eq!(z[0], 0x78);
        assert!(z.len() < data.len() / 10);
        assert_eq!(deflate_stream(6, b"").map(|z| z.len()), Some(8));
    }
}
