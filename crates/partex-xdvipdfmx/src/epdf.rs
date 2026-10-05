//! epdf.c, epdf.h: including a page of a PDF file as a form XObject, and
//! `pdf_copy_clip` (a page's clipping path copied into the output).
//!
//! A `pdf_file *` is the `pf: u32` of `self.o.pdf_open(ident, data)`
//! (pdfread.rs); the file's bytes are `fp.data`.

use crate::dpxutil::DpxStack;
use crate::prelude::*;

/// `pdfbox_crop`.
pub const PDFBOX_CROP: i32 = 1;
/// `pdfbox_media`.
pub const PDFBOX_MEDIA: i32 = 2;
/// `pdfbox_bleed`.
pub const PDFBOX_BLEED: i32 = 3;
/// `pdfbox_trim`.
pub const PDFBOX_TRIM: i32 = 4;
/// `pdfbox_art`.
pub const PDFBOX_ART: i32 = 5;

/// `enum action` (pdf_copy_clip's operators).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Unknown,
    Discard,
    Path,
    Rect,
    Trans,
    Clip,
    Save,
    Restore,
}

/// `struct operator`.
#[derive(Clone, Copy, Debug)]
pub struct Operator {
    pub token: &'static [u8],
    pub action: Action,
    pub n_args: i32,
}

/// `operators[]`, in C's order (two-character tokens first).
pub static OPERATORS: [Operator; 22] = [
    Operator {
        token: b"b*",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"B*",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"cm",
        action: Action::Trans,
        n_args: 6,
    },
    Operator {
        token: b"f*",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"re",
        action: Action::Rect,
        n_args: 4,
    },
    Operator {
        token: b"W*",
        action: Action::Path,
        n_args: 0,
    },
    Operator {
        token: b"b",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"B",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"c",
        action: Action::Path,
        n_args: 6,
    },
    Operator {
        token: b"f",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"F",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"h",
        action: Action::Path,
        n_args: 0,
    },
    Operator {
        token: b"l",
        action: Action::Path,
        n_args: 2,
    },
    Operator {
        token: b"m",
        action: Action::Path,
        n_args: 2,
    },
    Operator {
        token: b"n",
        action: Action::Path,
        n_args: 0,
    },
    Operator {
        token: b"q",
        action: Action::Save,
        n_args: 0,
    },
    Operator {
        token: b"Q",
        action: Action::Restore,
        n_args: 0,
    },
    Operator {
        token: b"s",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"S",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"v",
        action: Action::Path,
        n_args: 4,
    },
    Operator {
        token: b"W",
        action: Action::Path,
        n_args: 0,
    },
    Operator {
        token: b"y",
        action: Action::Path,
        n_args: 4,
    },
];

impl Dpx {
    /// `get_page_content` (static): the page's contents as one stream
    /// (an array of streams concatenated), or none.
    pub fn get_page_content(&mut self, pf: u32, page: Obj) -> Option<Obj> {
        todo!()
    }
    /// `pdf_include_page`: fills XObject `xobj_id` with page
    /// `options.page_no` of the PDF in `fp` (opened as `ident`); 0 or -1.
    pub fn pdf_include_page(
        &mut self,
        xobj_id: i32,
        fp: &mut MemFile,
        ident: &[u8],
        options: crate::pdfximage::LoadOptions,
    ) -> i32 {
        todo!()
    }
    /// `get_numbers_from_stack` (static): pops `n` numbers into
    /// `v[0..n]` (last popped first); 0 or -1. Popped objects are released.
    pub fn get_numbers_from_stack(
        &mut self,
        stack: &mut DpxStack<Obj>,
        v: &mut [f64],
        n: i32,
    ) -> i32 {
        todo!()
    }
    /// `pdf_copy_clip`: 0 or -1.
    pub fn pdf_copy_clip(
        &mut self,
        fp: &mut MemFile,
        page_index: i32,
        x_user: f64,
        y_user: f64,
    ) -> i32 {
        todo!()
    }
}
