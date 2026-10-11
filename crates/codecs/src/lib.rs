//! Pure codecs and readers, no TeX state: zlib's inflate, MD5, PNG
//! reading (libpng's semantics, as pdfTeX's `writepng` sees them), a PDF
//! reader and the text of its pages. Used by `phitex-draw` (the viewer's
//! pages), `phitex-xdvipdfmx` and the CLI.
//!
//! `no_std` + `alloc`.

#![no_std]
#![allow(clippy::unreadable_literal, clippy::many_single_char_names)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

extern crate alloc;

/// The pure-Rust port of zlib's deflate (byte-identical to pdfTeX's streams).
pub mod deflate;
pub mod inflate;
pub mod md5;
pub mod pdfread;
pub mod pdftext;
pub mod png;
pub mod zlib;
