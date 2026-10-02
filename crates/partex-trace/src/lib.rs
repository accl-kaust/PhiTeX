//! Tracing facade for partex.
//!
//! With the `trace` feature, [`span!`] enters a `tracing` span for the rest of the
//! enclosing scope. Without it, the macro expands to nothing, so instrumented code
//! costs nothing in normal builds. Works in `no_std` crates.

#![no_std]

#[cfg(feature = "trace")]
#[doc(hidden)]
pub use tracing as __tracing;

/// Enter a span named `$name` (a phase: `"expand"`, `"line_break"`, `"shipout"`, …)
/// until the end of the enclosing block.
#[cfg(feature = "trace")]
#[macro_export]
macro_rules! span {
    ($name:literal $(, $($fields:tt)*)?) => {
        let __partex_span = $crate::__tracing::info_span!($name $(, $($fields)*)?);
        let __partex_guard = __partex_span.enter();
    };
}

/// Enter a span named `$name` (a phase: `"expand"`, `"line_break"`, `"shipout"`, …)
/// until the end of the enclosing block.
#[cfg(not(feature = "trace"))]
#[macro_export]
macro_rules! span {
    ($name:literal $(, $($fields:tt)*)?) => {};
}
