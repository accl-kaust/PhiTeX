//! phitex-defuse's tests.
//! (one test program for the crate's test files, each a module: a
//! program each was linked whole, uncached, every gate run)

mod absorb_running;
mod alias;
mod before_placer;
mod block_heads;
mod chains;
mod claims;
mod collect;
mod collect_review;
mod compact;
mod core;
mod derived;
mod filed_reads;
mod final_only;
mod fixes;
mod keep_longest;
mod loopback;
mod merged_wake;
mod park;
mod park_review;
mod placed_edit;
mod placed_guess;
mod predict;
mod ranked;
mod relocate;
mod resumed_reread;
mod review;
mod review2;
mod scope_races;
mod scopes;
mod settle;
mod settled;
mod snapshot;
mod stands;
mod steps;
mod stood;
mod stress;
mod wasted;
mod within;
