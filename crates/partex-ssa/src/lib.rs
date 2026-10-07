//! Dynamic SSA (`DESIGN.md` §7.17): the build as one graph.
//!
//! Every operation is a call `v = f(a₁ … aₙ)`, every piece of state an
//! immutable value whose version is its content, and a call runs again
//! iff a version it read changed. The runtime knows nothing of TeX; a
//! language implements [`Machine`] and writes its bodies against [`Cx`].
//!
//! - [`hash`]: versions, stable 128-bit content hashes.
//! - [`value`], [`pvec`], [`pmap`], [`pstack`]: values and persistent
//!   containers with Merkle versions and structural sharing (7.17.1).
//! - [`machine`]: the language interface, addresses and reads.
//! - [`runtime`]: records, names by content, reads verified in order,
//!   stores and loads with the φ, the trips of a build (7.17.2–7.17.7).
//! - [`open`]: the runtime opened to a language that owns its state
//!   (a [`Store`]), with re-entrant calls and a recorder driven from
//!   outside (the engine's tracker).
//! - [`fold`]: the top level as a fold of steps, each slot's definitions
//!   and readers (7.17.3).
//! - [`trace`]: the trace's text form, LLVM-IR shaped (§9).
//! - [`stub`]: a small language exercising all of it, and its oracle.
//!
//! `no_std` + `alloc`; the `std` feature adds a wall clock.

#![no_std]
#![allow(clippy::many_single_char_names, clippy::similar_names)]
// The panics are broken invariants (a dead record id, an index past the
// end), never input errors.
#![allow(clippy::missing_panics_doc)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod export;
pub mod fold;
pub mod hash;
pub mod machine;
pub mod open;
pub mod pmap;
pub mod pstack;
pub mod pvec;
pub mod runtime;
pub mod stub;
pub mod table;
pub mod trace;
pub mod value;

pub use hash::Version;
pub use machine::{Cx, Loc, Machine, Stream, name_of};
pub use open::{Found, MapStore, Store};
pub use pmap::PMap;
pub use pstack::PStack;
pub use pvec::PVec;
pub use runtime::{Config, EffectKind, Outcome, Runtime, Stats, Status, TripReport};
pub use trace::Trace;
pub use value::Value;

/// Nanoseconds since the first call, for [`Config::clock`].
#[cfg(feature = "std")]
#[must_use]
pub fn std_clock() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static T0: OnceLock<Instant> = OnceLock::new();
    let t0 = T0.get_or_init(Instant::now);
    u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
