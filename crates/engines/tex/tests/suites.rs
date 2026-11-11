//! phitex-tex's suites (the gate's suites shard): pdftex, edits, pages,
//! `page_steps`, plain, etex, hints, inputs.
//! (one test program for the crate's test files, each a module: a
//! program each was linked whole, uncached, every gate run)

mod common;
mod edits;
mod etex;
mod hints;
mod inputs;
mod page_steps;
mod pages;
mod pdftex;
mod plain;
