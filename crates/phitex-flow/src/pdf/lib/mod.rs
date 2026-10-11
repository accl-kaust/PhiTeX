//! Pure libraries copied from partex-engine (the wall, `tests/wall.rs`:
//! copied as they are, never linked): xpdf's reader of PDF files
//! (`pdfread`, with `inflate` and the font-file readers `fofi`,
//! `gfxfont`), libpng's reading (`png`), the stored zlib container (`zstored`, for the
//! tests).

#[allow(clippy::all, clippy::pedantic)]
pub mod fofi;
#[allow(clippy::all, clippy::pedantic)]
pub mod fofi_tables;
#[allow(clippy::all, clippy::pedantic)]
pub mod gfxfont;
#[allow(clippy::all, clippy::pedantic)]
pub mod inflate;
#[allow(clippy::all, clippy::pedantic)]
pub mod pdfread;
#[allow(clippy::all, clippy::pedantic)]
pub mod png;
#[allow(clippy::all, clippy::pedantic)]
#[allow(clippy::all, clippy::pedantic)]
pub mod zstored;
