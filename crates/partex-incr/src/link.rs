//! Effects as values and the link step (`DESIGN.md` §7.6).
//!
//! Every region's output is a chunk of effect values. The link step
//! assembles the file in program order, as a linker does. This generic
//! link (the runtime's and the stub's) relinks every chunk each time
//! (mold's argument: a fast full link beats splicing), which was the
//! policy when a rebuild re-ran regions by the hundreds.
//!
//! The engine's link (`partex-core`, `effects.rs`) costs what changed:
//! a rebuild whose regions made the same effects as before links
//! nothing and keeps the files (the build's `effects_changed`); otherwise
//! only the regions put in are resolved again (`link_cached`), the
//! others taken from the last link, and only the files whose bytes
//! changed are written (DESIGN.md §7.16.6; `PARTEX_LINK_SPLICE=0` in the
//! in-process path, `PARTEX_MACHINE_LINK_CACHE=0` and
//! `PARTEX_MACHINE_LINK_REUSE=0` in a watch, for the full link):
//!
//! 1. symbol bases by a prefix sum over each chunk's allocation count;
//! 2. every chunk rendered on its own, in parallel, holes resolved by the
//!    chunk's environment and forward references by the final state;
//! 3. offsets by a prefix sum over the rendered lengths;
//! 4. every chunk copied, in parallel, into one preallocated buffer.

use alloc::vec;
use alloc::vec::Vec;

use crate::exec::Executor;
use crate::machine::{Hole, Machine};

/// One region's output.
#[derive(Debug)]
pub struct Chunk<'a, M: Machine> {
    pub effects: &'a [M::Effect],
    /// Hole `i` of these effects has value `env[i]`.
    pub env: &'a [M::Value],
    /// Symbols the effects allocate (the sum of [`Machine::allocs`]).
    pub allocs: u64,
}

/// What an effect sees while it is rendered.
pub struct LinkCtx<'a, M: Machine> {
    /// The state at the end of the run: forward references read it.
    pub final_state: &'a M,
    env: &'a [M::Value],
    next_symbol: u64,
}

impl<M: Machine> LinkCtx<'_, M> {
    /// The next link-time symbol, in allocation order over the whole run
    /// (0-based).
    pub fn alloc(&mut self) -> u64 {
        let n = self.next_symbol;
        self.next_symbol += 1;
        n
    }

    /// The value of hole `h` of the chunk being rendered.
    #[must_use]
    pub fn hole(&self, h: Hole) -> Option<&M::Value> {
        self.env.get(h.0 as usize)
    }
}

/// Link `chunks` into the output.
pub fn link<M: Machine, E: Executor>(
    chunks: &[Chunk<'_, M>],
    final_state: &M,
    exec: &E,
) -> Vec<u8> {
    // 1. Symbol bases.
    let mut bases = Vec::with_capacity(chunks.len());
    let mut base = 0u64;
    for c in chunks {
        bases.push(base);
        base += c.allocs;
    }
    // 2. Render, in batches so a thread's work outweighs its bookkeeping.
    let batch = chunks.len().div_ceil(exec.width() * 8).max(1);
    let jobs: Vec<usize> = (0..chunks.len()).step_by(batch).collect();
    let rendered: Vec<Vec<Vec<u8>>> = exec.map(jobs, |start| {
        let end = (start + batch).min(chunks.len());
        (start..end)
            .map(|i| {
                let c = &chunks[i];
                let mut cx = LinkCtx {
                    final_state,
                    env: c.env,
                    next_symbol: bases[i],
                };
                let mut out = Vec::new();
                for e in c.effects {
                    M::render(e, &mut cx, &mut out);
                }
                debug_assert_eq!(
                    cx.next_symbol,
                    bases[i] + c.allocs,
                    "allocs() and render() disagree"
                );
                out
            })
            .collect()
    });
    let pieces: Vec<&[u8]> = rendered.iter().flatten().map(Vec::as_slice).collect();
    // 3. Offsets.
    let total: usize = pieces.iter().map(|p| p.len()).sum();
    // 4. Copy into disjoint parts of one buffer.
    let mut out = vec![0u8; total];
    let mut rest: &mut [u8] = &mut out;
    let mut parts: Vec<(&mut [u8], &[u8])> = Vec::with_capacity(pieces.len());
    for p in &pieces {
        let (dst, tail) = rest.split_at_mut(p.len());
        parts.push((dst, p));
        rest = tail;
    }
    if total >= 1 << 20 {
        exec.map(parts, |(dst, src)| dst.copy_from_slice(src));
    } else {
        for (dst, src) in parts {
            dst.copy_from_slice(src);
        }
    }
    out
}
