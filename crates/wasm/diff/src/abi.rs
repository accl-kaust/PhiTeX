//! The host's side: its request bytes in, the reply's out.

use std::sync::{Mutex, PoisonError};

/// The last reply: the host copies it out (`ph_out`, the length `ph_diff`
/// gave) before its next call.
static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// `n` bytes for the host to write a request into.
#[unsafe(no_mangle)]
pub extern "C" fn ph_alloc(n: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(n);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Give back what `ph_alloc` gave.
///
/// # Safety
/// `p` and `n` as `ph_alloc` gave them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ph_free(p: *mut u8, n: usize) {
    // SAFETY: `p` is a `Vec<u8>`'s buffer of capacity `n` (`ph_alloc`), length 0: nothing to drop but the buffer.
    drop(unsafe { Vec::from_raw_parts(p, 0, n) });
}

/// Diff the request in `[p, p + n)` (which this frees); the reply's length (read it with `ph_out`).
///
/// # Safety
/// `p` and `n` as `ph_alloc` gave them, the request written in.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ph_diff(p: *mut u8, n: usize) -> usize {
    let out = {
        // SAFETY: `p` is `ph_alloc(n)`'s buffer, its `n` bytes written by the host.
        let req = unsafe { std::slice::from_raw_parts(p, n) };
        serde_json::to_vec(&super::answer(req)).unwrap_or_default()
    };
    // SAFETY: as `ph_free`'s: the request read, its buffer given back.
    unsafe { ph_free(p, n) };
    let mut o = OUT.lock().unwrap_or_else(PoisonError::into_inner);
    *o = out;
    o.len()
}

/// Where the last reply is.
#[unsafe(no_mangle)]
pub extern "C" fn ph_out() -> *const u8 {
    OUT.lock().unwrap_or_else(PoisonError::into_inner).as_ptr()
}
