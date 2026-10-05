//! The program's state: dvipdfm-x's globals, one field per C file.
//!
//! Every C function that reads or writes a global (an object, a cache, a
//! file) is a method of [`Dpx`] in the module of its C file; a function
//! of plain data is a function or a method of that data. A C file's
//! `static` variables are the fields of its module's `State`.

use alloc::boxed::Box;

use crate::io::Files;
use crate::obj::PdfOut;

/// `dpx_conf` (dpxconf.h).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CompatMode {
    #[default]
    Normal,
    Compat,
    Xdv,
    Mpost,
}

#[derive(Clone, Debug, Default)]
pub struct DpxConf {
    pub is_xbb: bool,
    pub verbose_level: i32,
    pub compat_mode: CompatMode,
    pub ignore_font_license: bool,
    pub pdfm_str_utf8: bool,
}

/// The whole program.
pub struct Dpx {
    /// The objects and the writer (pdfobj.c).
    pub o: PdfOut,
    /// The host's files (dpxfile.c's kpathsea).
    pub files: Box<dyn Files>,
    pub conf: DpxConf,
    /// dvipdfmx.c's `dvi_filename` and `pdf_filename` (subset tags hash
    /// them; xelatex pipes the XDV in, so the first is none).
    pub dvi_filename: Option<alloc::vec::Vec<u8>>,
    pub pdf_filename: Option<alloc::vec::Vec<u8>>,

    pub dvi: crate::dvi::State,
    pub dev: crate::pdfdev::State,
    pub doc: crate::pdfdoc::State,
    pub draw: crate::pdfdraw::State,
    pub color: crate::pdfcolor::State,
    pub resource: crate::pdfresource::State,
    pub font: crate::pdffont::State,
    pub fontmap: crate::fontmap::State,
    pub tfm: crate::tfm::State,
    pub vf: crate::vf::State,
    pub agl: crate::agl::State,
    pub encoding: crate::pdfencoding::State,
    pub cmap: crate::cmap::State,
    pub cid: crate::cid::State,
    pub ximage: crate::pdfximage::State,
    pub spc: crate::specials::State,
    pub pdfm: crate::spc_pdfm::State,
    pub xtx: crate::spc_xtx::State,
    pub misc: crate::spc_misc::State,
    pub html: crate::spc_html::State,
    pub t1_char: crate::t1_char::State,
    pub cs_type2: crate::cs_type2::State,
    pub session: crate::session::State,
}

/// dvipdfm-x's `WARN`: dropped (the transcript is not output). The
/// arguments are still evaluated, as in C.
#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => {{
        let _ = ::core::format_args!($($t)*);
    }};
}

/// dvipdfm-x's `ERROR`: the run stops.
#[macro_export]
macro_rules! error {
    ($($t:tt)*) => {
        ::core::panic!($($t)*)
    };
}

impl Dpx {
    /// A fresh program: every C static at its initial value.
    #[must_use]
    pub fn new(files: Box<dyn Files>, deflate: crate::obj::Deflate) -> Self {
        Dpx {
            o: PdfOut::new(deflate),
            files,
            conf: DpxConf::default(),
            dvi_filename: None,
            pdf_filename: None,
            dvi: Default::default(),
            dev: Default::default(),
            doc: Default::default(),
            draw: Default::default(),
            color: Default::default(),
            resource: Default::default(),
            font: Default::default(),
            fontmap: Default::default(),
            tfm: Default::default(),
            vf: Default::default(),
            agl: Default::default(),
            encoding: Default::default(),
            cmap: Default::default(),
            cid: Default::default(),
            ximage: Default::default(),
            spc: Default::default(),
            pdfm: Default::default(),
            xtx: Default::default(),
            misc: Default::default(),
            html: Default::default(),
            t1_char: Default::default(),
            cs_type2: Default::default(),
            session: Default::default(),
        }
    }
}
