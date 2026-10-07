//! A language-independent runtime for incremental and parallel execution
//! (`DESIGN.md` §7.0).
//!
//! The runtime knows nothing about TeX. A language plugs in by
//! implementing [`Machine`]: a deterministic interpreter over *cells*
//! (units of state with hashable values) that reports every read and
//! write to a [`Recorder`], emits its output as effect values, and says
//! where a region may begin or end. Everything else lives here:
//!
//! - [`hash`]: stable 128-bit content hashes; a cell's version is the hash
//!   of its value (§7.1).
//! - [`trace`]: region traces, guards (first reads with versions) plus
//!   effects (net writes and emitted output), recorded at dynamically
//!   chosen boundaries (§7.3).
//! - [`build`]: a recorded build with backedges from cells to the regions
//!   that read them, and program-order propagation with early cutoff and
//!   read-set cutoff after an edit (§7.4).
//! - [`link`]: the output of every region is a chunk of effect values; the
//!   link step renders them in parallel, lays them out by prefix sums and
//!   copies them into one preallocated buffer (§7.6). Symbolic numbering
//!   and forward references resolve there.
//! - [`rounds`]: parallel Jacobi rounds from composed guesses, with holes
//!   (affine `h + k` values), forcing points and the suspend and
//!   speculate policies, iterated to the sequential fixpoint (§7.5, §7.7).
//! - [`exec`]: the executor, sequential or threaded.
//! - [`stub`]: a tiny line-oriented language that drives the tests.
//!
//! Every acceleration is switchable ([`build::Config`],
//! [`rounds::Config`], the executor); every switch-off path gives the
//! output of [`run_plain`], the plain sequential run.
//!
//! `no_std` + `alloc`; the `threads` feature adds the threaded executor.

#![no_std]
// Interpreter and scheduler code keeps the conventional short names: `m` a
// machine, `r` a recorder, `s` a state, `d` the difference set, `c` a cell.
#![allow(clippy::many_single_char_names, clippy::similar_names)]

extern crate alloc;
#[cfg(feature = "threads")]
extern crate std;

pub mod build;
pub mod exec;
pub mod hash;
pub mod link;
pub mod machine;
pub mod rounds;
pub mod stub;
pub mod trace;

pub use build::{Audit, Build, Differ, Index, Later, MakeLater};
#[cfg(feature = "threads")]
pub use exec::Threads;
pub use exec::{Executor, Sequential};
pub use hash::{Version, version_of};
pub use link::{Chunk, LinkCtx, link};
pub use machine::{Affine, Forced, Hole, LAYER, Machine, Recorder, Shift, Split, Step};
pub use trace::Trace;

use alloc::vec::Vec;

/// Run `m` to its end with no recording: the reference every accelerated
/// path must reproduce byte for byte. Returns the final state and the
/// linked output.
///
/// # Panics
///
/// If the machine suspends: nothing planted a hole, so nothing can force
/// one.
pub fn run_plain<M: Machine>(mut m: M) -> (M, Vec<u8>) {
    let mut rec = trace::Plain::<M> {
        effects: Vec::new(),
    };
    loop {
        match m.step(&mut rec) {
            Step::Continue | Step::Candidate(_) => {}
            Step::Halt => break,
            Step::Suspended => panic!("a plain run cannot suspend: it has no holes"),
        }
    }
    let allocs = rec.effects.iter().map(M::allocs).sum();
    let chunk = Chunk {
        effects: &rec.effects,
        env: &[],
        allocs,
    };
    let out = link(&[chunk], &m, &Sequential);
    (m, out)
}
