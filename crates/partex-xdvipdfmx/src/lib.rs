//! xdvipdfmx (dvipdfm-x 20260113, as TeX Live 2026 builds it) ported:
//! XDV in, PDF out, byte for byte what `xelatex` writes.
//!
//! `no_std` + `alloc`: files come through [`Files`], compression through
//! the caller's deflater.

// Skeleton pass: bodies are todo!() for now.
#![allow(unused, dead_code, clippy::all, clippy::pedantic)]
#![no_std]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::unreadable_literal,
    clippy::float_cmp,
    clippy::missing_panics_doc,
    clippy::struct_excessive_bools,
    clippy::fn_params_excessive_bools,
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::bool_to_int_with_if,
    clippy::if_not_else,
    clippy::manual_range_contains,
    clippy::wildcard_imports
)]

extern crate alloc;

pub mod agl;
pub mod api;
pub mod cff;
pub mod cff_dict;
pub mod cid;
pub mod cidtype0;
pub mod cidtype2;
pub mod cmap;
pub mod cmap_read;
pub mod cmap_write;
pub mod cs_type2;
pub mod ctx;
pub mod dpxfile;
pub mod dpxutil;
pub mod dvi;
pub mod epdf;
pub mod filter;
pub mod fmt;
pub mod fontmap;
pub mod io;
pub mod jpegimage;
pub mod obj;
pub mod otl_opt;
pub mod parse;
pub mod pdfcolor;
pub mod pdfdev;
pub mod pdfdoc;
pub mod pdfdraw;
pub mod pdfencoding;
pub mod pdffont;
pub mod pdfnames;
pub mod pdfread;
pub mod pdfresource;
pub mod pdfximage;
pub mod pngimage;
pub mod pst;
pub mod pst_obj;
pub mod session;
pub mod sfnt;
pub mod spc_color;
pub mod spc_dvipdfmx;
pub mod spc_html;
pub mod spc_misc;
pub mod spc_pdfm;
pub mod spc_util;
pub mod spc_xtx;
pub mod specials;
pub mod stream;
pub mod t1_char;
pub mod t1_load;
pub mod tfm;
pub mod truetype;
pub mod tt_aux;
pub mod tt_cmap;
pub mod tt_glyf;
pub mod tt_gsub;
pub mod tt_post;
pub mod tt_table;
pub mod type0;
pub mod type1;
pub mod type1c;
pub mod unicode;
pub mod vf;

/// What every module uses.
pub mod prelude {
    pub use alloc::boxed::Box;
    pub use alloc::format;
    pub use alloc::rc::Rc;
    pub use alloc::string::String;
    pub use alloc::vec;
    pub use alloc::vec::Vec;

    pub use crate::ctx::Dpx;
    pub use crate::obj::{Obj, PdfOut};
    pub use crate::stream::MemFile;
    pub use crate::{error, warn};
}

/// `PDF_VERSION_MIN`, `PDF_VERSION_MAX` (pdflimits.h).
pub const PDF_VERSION_MIN: i32 = 13;
pub const PDF_VERSION_MAX: i32 = 20;
