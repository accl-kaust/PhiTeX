//! The heap's size, chosen once at the start
//! (`docs/note-browser-memory.md`, option B). The module is linked with 2 GiB
//! of initial memory and at most 4 GiB less 64 KiB (`build.rs`); the
//! host makes the shared memory at 2 GiB and, on thread 0 before the first
//! thread is spawned, asks `ph_grow` for 4 GiB, then 3, then keeps 2
//! (common/src/wasm/threads.ts `chooseSize`), and then `ph_seal`s it: from
//! there on the memory never grows. Growing a shared memory while other
//! threads are inside wasm traps in V8 (their memory.fill into the new
//! pages), and the engine's threads park inside wasm; with one thread no
//! other can be in it.
//!
//! The growth is the allocator's own. wasi-libc's dlmalloc takes its first
//! region from `__heap_base` to `__heap_end` (the link's initial memory)
//! and grows past it by `sbrk`, which is `memory.grow`: memory grown from
//! the host would lie outside its heap. So `ph_grow` allocates blocks
//! (dlmalloc's `sbrk`s, each one `memory.grow` that either succeeds whole
//! or changes nothing) until the memory has the size asked for, and frees
//! them: dlmalloc never gives memory back (wasm memory cannot shrink), so
//! the grown pages stay its free top.
//!
//! The seal is this module's `sbrk`, which wasi-libc's dlmalloc calls in
//! place of libc's (the linker takes the first definition; libc's
//! `sbrk.c.obj` defines nothing else): libc's, refusing to grow once
//! sealed. A document that outgrows the size then fails an allocation
//! ("memory allocation of N bytes failed", the core aborts) and the host
//! says what size the tab got and what to close (threads.ts
//! `outgrownMessage`).

use std::alloc::{GlobalAlloc, Layout, System};
use std::arch::wasm32;
use std::sync::atomic::{AtomicBool, Ordering};

const PAGE: usize = 65536;
/// 64 KiB short of 4 GiB: at 4 GiB the heap's end would wrap to 0.
const MAX_PAGES: usize = 65535;

static SEALED: AtomicBool = AtomicBool::new(false);

/// wasi-libc's `sbrk` (`libc-bottom-half/sources/sbrk.c`), refusing to grow
/// once [`ph_seal`]ed (dlmalloc's allocation then fails: NULL).
#[unsafe(no_mangle)]
pub extern "C" fn sbrk(increment: isize) -> *mut u8 {
    const FAIL: *mut u8 = usize::MAX as *mut u8;
    if increment == 0 {
        return (wasm32::memory_size::<0>() * PAGE) as *mut u8;
    }
    // (as libc's: page-sized increments only, and wasm memory never shrinks)
    if increment < 0 || !increment.unsigned_abs().is_multiple_of(PAGE) {
        std::process::abort();
    }
    if SEALED.load(Ordering::Acquire) {
        out_of_memory();
        return FAIL;
    }
    let old = wasm32::memory_grow::<0>(increment.unsigned_abs() / PAGE);
    if old == usize::MAX {
        return FAIL;
    }
    (old * PAGE) as *mut u8
}

/// The document outgrew the memory (sealed: dlmalloc's allocation fails
/// next, and the core aborts, "memory allocation of N bytes failed"):
/// told to the host first, once, as the event `{"event":"lost",...}`
/// ([`crate::session::LOST_MEMORY`]), so that a host whose threads' trap
/// never reaches it still says why the document is gone. Inside dlmalloc,
/// its lock held: nothing allocated here (a static text, the host's
/// import).
fn out_of_memory() {
    static TOLD: AtomicBool = AtomicBool::new(false);
    if !TOLD.swap(true, Ordering::AcqRel) {
        crate::abi::event(crate::session::LOST_MEMORY);
    }
}

/// The memory grown by the allocator towards `pages` (at most 65535):
/// blocks taken until it is within 3 pages of it, or one is refused, then
/// freed. The memory's pages after. Thread 0 only, before any thread is
/// spawned; nothing once sealed.
#[unsafe(no_mangle)]
pub extern "C" fn ph_grow(pages: usize) -> usize {
    let want = pages.min(MAX_PAGES) * PAGE;
    let mut held = [(std::ptr::null_mut::<u8>(), 0usize); 16];
    let mut n = 0;
    while n < held.len() && !SEALED.load(Ordering::Acquire) {
        let size = wasm32::memory_size::<0>() * PAGE;
        if size + 3 * PAGE >= want {
            break;
        }
        // (short of it by two pages: dlmalloc's sbrk is the block less its
        // free top, rounded up to a page, so it never asks past `want`)
        let ask = (want - size - 2 * PAGE).min(isize::MAX as usize - PAGE);
        let Ok(l) = Layout::from_size_align(ask, 16) else {
            break;
        };
        // SAFETY: a layout of non-zero size.
        let p = unsafe { System.alloc(l) };
        if p.is_null() {
            break;
        }
        held[n] = (p, ask);
        n += 1;
    }
    for &(p, ask) in &held[..n] {
        // SAFETY: `p` was allocated by `System` with this layout above.
        unsafe { System.dealloc(p, Layout::from_size_align_unchecked(ask, 16)) };
    }
    wasm32::memory_size::<0>()
}

/// The size kept from now on: the memory never grows again.
#[unsafe(no_mangle)]
pub extern "C" fn ph_seal() {
    SEALED.store(true, Ordering::Release);
}
