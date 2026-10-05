//! The C library's heap, given back after the SSA collector freed much
//! of it (`malloc_trim`, glibc).
//!
//! A collection frees millions of small values among live data. glibc
//! keeps them in its bins unsorted until a later allocation walks them:
//! the walk falls on the next keystrokes' allocations, the link's first
//! (a thesis's link 5 ms became 7 to 25 ms for a dozen keystrokes after
//! the build). Trimming once, right after a build that collected, sorts
//! and coalesces them then, and gives the free pages back to the system.
#![expect(unsafe_code, reason = "FFI to the C library's malloc_trim")]

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

/// Consolidate the heap's free chunks and return free pages (a no-op
/// where the C library has no `malloc_trim`).
pub fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim takes no pointers and is thread-safe.
    unsafe {
        malloc_trim(0);
    }
}
