//! (`memstats`) The core's allocations counted, for measuring its
//! footprint: the global allocator is the system's (wasi-libc's dlmalloc
//! on wasm32) counting, on every thread, the bytes live, their peak and
//! the highest end of any block it handed out (the heap's high-water: on
//! the fixed memory, what the memory must hold). The host reads them at any
//! time at `ph_mem()`: four u32 words, live, peak, high-water end, the
//! allocations made, without calling in.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub struct Counted;

static STATS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
const LIVE: usize = 0;
const PEAK: usize = 1;
const END: usize = 2;
const COUNT: usize = 3;

fn took(p: *mut u8, n: usize) {
    if !p.is_null() {
        let live = STATS[LIVE].fetch_add(n, Relaxed) + n;
        STATS[PEAK].fetch_max(live, Relaxed);
        STATS[END].fetch_max(p as usize + n, Relaxed);
        STATS[COUNT].fetch_add(1, Relaxed);
    }
}

// SAFETY: every call is the system allocator's, with the same arguments.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: as the caller's.
        let p = unsafe { System.alloc(l) };
        took(p, l.size());
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        // SAFETY: as the caller's.
        let p = unsafe { System.alloc_zeroed(l) };
        took(p, l.size());
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        STATS[LIVE].fetch_sub(l.size(), Relaxed);
        // SAFETY: as the caller's.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        // SAFETY: as the caller's.
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            STATS[LIVE].fetch_sub(l.size(), Relaxed);
            took(q, n);
        }
        q
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ph_mem() -> *const AtomicUsize {
    STATS.as_ptr()
}
