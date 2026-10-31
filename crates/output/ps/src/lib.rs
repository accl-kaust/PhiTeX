//! PostScript to PDF as Ghostscript 10.08.0's pdfwrite writes it: the
//! converter TeX Live runs for EPS figures, ported so that PhiTeX runs
//! no program.
//!
//! - pdfLaTeX: `repstopdf` (epstopdf.pl) rewrites the EPS header
//!   ([`epstopdf_feed`]) and pipes it to `gs ... -dPDFSETTINGS=/prepress
//!   -sDEVICE=pdfwrite -`: [`Settings::epstopdf`].
//! - XeLaTeX: xdvipdfmx's dvipdfmx.cfg `D` line, `gs -dEPSCrop
//!   -sPAPERSIZE=a0 ... file.eps`: [`Settings::dvipdfmx`].
//!
//! [`convert`] runs the program in a PostScript interpreter (Level 2/3,
//! Type 1 fonts from Ghostscript's own `Resource/Font`, read by name
//! through [`Resources`]) and writes the page as pdfwrite does: its
//! content stream byte for byte, its resources and object numbers, the
//! file's layout. Two values are not reproduced (neither reaches the
//! document that includes the figure): the trailer's `/ID` and the XMP
//! `DocumentID`'s node (gs hashes its own process state into them).
//!
//! `no_std` + `alloc`.

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
    clippy::missing_errors_doc,
    clippy::struct_excessive_bools,
    clippy::fn_params_excessive_bools,
    clippy::too_many_arguments,
    clippy::needless_range_loop,
    clippy::bool_to_int_with_if,
    clippy::if_not_else,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::doc_markdown,
    clippy::items_after_statements,
    clippy::match_same_arms,
    clippy::wildcard_imports,
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value,
    clippy::new_without_default,
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::manual_range_contains,
    clippy::collapsible_if,
    clippy::collapsible_else_if,
    clippy::single_match_else,
    clippy::redundant_else,
    clippy::trivially_copy_pass_by_ref,
    clippy::ref_option,
    clippy::used_underscore_binding,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_self,
    clippy::semicolon_if_nothing_returned,
    clippy::explicit_iter_loop,
    clippy::uninlined_format_args,
    clippy::manual_let_else,
    clippy::option_if_let_else,
    clippy::large_types_passed_by_value,
    clippy::comparison_chain,
    clippy::neg_multiply,
    clippy::suboptimal_flops,
    clippy::imprecise_flops
)]

extern crate alloc;

mod cff;
mod color;
mod device;
mod enc;
mod feed;
mod file;
mod flatten;
mod font;
mod gsnames;
mod gstate;
mod image;
mod init;
mod interp;
mod matrix;
mod names;
mod num;
mod obj;
mod paint;
mod path;
mod pattern;
mod pdf;
mod pdfcolor;
mod pdffont;
mod scan;
mod text;
mod uni;
mod upath;
mod xmp;

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

pub use feed::{epstopdf_feed, perl_num};
pub use interp::Resources;
pub use pdf::Settings;

/// What a conversion gives: the PDF, and Ghostscript's error message if
/// the program stopped on one (gs then still writes what was drawn, and
/// exits nonzero).
pub struct Output {
    pub pdf: Vec<u8>,
    pub error: Option<String>,
}

/// The base 35 fonts' file names in Ghostscript's `Resource/Font`: what
/// a [`Resources`] serves.
pub fn font_resource_names() -> impl Iterator<Item = &'static str> {
    font::FONTMAP.iter().map(|(_, f)| *f)
}

impl Settings {
    /// epstopdf.pl's gs command (`-dPDFSETTINGS=/prepress`, the input on
    /// stdin), writing `out_name`.
    pub fn epstopdf(epoch: i64) -> Settings {
        let inv: [&str; 15] = [
            "gs",
            "-q",
            "-dSAFER",
            "-dNOPAUSE",
            "-dBATCH",
            "-dCompatibilityLevel=1.5",
            "-sDEVICE=pdfwrite",
            "-sOutputFile=?",
            "-dPDFSETTINGS#/prepress",
            "-dMaxSubsetPct=100",
            "-dSubsetFonts=true",
            "-dEmbedAllFonts=true",
            "-dAutoRotatePages#/None",
            "-",
            "",
        ];
        Settings {
            level: 15,
            prepress: true,
            eps_crop: false,
            paper: (612.0, 792.0),
            flate_images: false,
            epoch,
            out_name: Vec::new(),
            invocation: inv[..14].iter().map(|s| s.as_bytes().to_vec()).collect(),
        }
    }

    /// dvipdfmx.cfg's `D` line (`-dEPSCrop -sPAPERSIZE=a0`, the images
    /// Flate-encoded), at PDF version `level` (15: 1.5).
    pub fn dvipdfmx(level: i32, epoch: i64) -> Settings {
        let lv = alloc::format!("-dCompatibilityLevel={}.{}", level / 10, level % 10);
        let inv: Vec<String> = [
            "gs",
            "-q",
            "-dSAFER",
            "-dNOPAUSE",
            "-dBATCH",
            "-dEPSCrop",
            "-sPAPERSIZE=a0",
            "-sDEVICE=pdfwrite",
            lv.as_str(),
            "-dAutoFilterGrayImages=false",
            "-dGrayImageFilter=/FlateEncode",
            "-dAutoFilterColorImages=false",
            "-dColorImageFilter=/FlateEncode",
            "-dAutoRotatePages=/None",
            "-sOutputFile=?",
            "?",
        ]
        .iter()
        .map(|s| String::from(*s))
        .collect();
        Settings {
            level,
            prepress: false,
            eps_crop: true,
            paper: (2384.0, 3370.0),
            flate_images: true,
            epoch,
            out_name: Vec::new(),
            invocation: inv.into_iter().map(String::into_bytes).collect(),
        }
    }
}

/// Converts PostScript program `input` as gs does with `s`: for the D
/// line `input` is the file as gs reads it (`run`: an EPSF header makes
/// it an EPS, cropped and wrapped in save/restore); for epstopdf it is
/// [`epstopdf_feed`]'s output read from stdin.
pub fn convert(input: &[u8], s: Settings, res: &dyn Resources) -> Output {
    let stdin = !s.eps_crop;
    let dev = pdf::Device::new(s);
    let mut ps = interp::Ps::new(res, dev);
    ps.initgraphics();
    let data = if stdin {
        input.to_vec()
    } else {
        feed::dos_eps_section(input)
    };
    let eps = !stdin && is_epsf(&data);
    ps.run_file(Rc::new(data), eps);
    let error = ps.failed.take();
    ps.dev.finish_page();
    pdffont::write_fonts(&mut ps);
    let pdf = ps.dev.document();
    Output { pdf, error }
}

/// gs's `run`: `%!PS-Adobe-` then, within the 23 bytes `%!PS-Adobe-X.X
/// EPSF-X.X` takes, `EPSF`.
fn is_epsf(d: &[u8]) -> bool {
    if d.len() < 23 || !d.starts_with(b"%!PS-Adobe-") {
        return false;
    }
    d[..23].windows(4).any(|w| w == b"EPSF")
}
