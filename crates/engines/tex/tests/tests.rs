//! phitex-tex's tests but latex and the suites (the gate's tests shard).
//! (one test program for the crate's test files, each a module: a
//! program each was linked whole, uncached, every gate run)

mod absorb;
mod auxloop;
mod chain;
mod common;
mod core;
mod cuts;
mod defer;
mod diagnostics;
mod edit_threads;
mod expand_depth;
mod fatal;
mod handles;
mod ini;
mod pages2;
mod pkgrec;
mod probe;
mod profile;
mod shell;
mod stale_loop;
mod state;
mod stuck;
mod tikzman_repros;
mod trip;
mod xetex;
