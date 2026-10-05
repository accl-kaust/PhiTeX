//! dvipdfmx.c's options, config file and page loop; dpxconf.c's paper
//! table.
//!
//! Not ported here: encryption (`get_enc_password`, `pdf_enc_*`), MetaPost
//! input (`do_mps_pages`, `mps_*`), extractbb (`xbb`), tpic
//! (`tpic_set_fill_mode`), the image cache (`dpx_delete_old_cache`),
//! kpathsea setup, usage/version messages. `dvi_filename` and
//! `pdf_filename` are `Dpx` fields (ctx.rs).

use crate::prelude::*;

/// `PACKAGE_VERSION` / `VERSION` as this tree's configure.ac sets it.
pub const VERSION: &[u8] = b"20260113";
/// The `PRODUCER` format's text after `"%s-%s"` (`my_name`, `VERSION`).
pub const PRODUCER_TAIL: &[u8] =
    b", Copyright 2002-2026 by Jin-Hwan Cho, Matthias Franz, and Shunsaku Hirata";
/// `DPX_CONFIG_FILE` (dpxfile.h).
pub const DPX_CONFIG_FILE: &[u8] = b"dvipdfmx.cfg";
/// `MAX_PWD_LEN` (pdfencrypt.h).
pub const MAX_PWD_LEN: usize = 127;
/// `optstrig`: the getopt option string.
pub const OPTSTRIG: &[u8] = b":hD:r:m:g:x:y:o:s:p:clf:i:qtvV:z:d:I:K:P:O:MSC:Ee";

pub const OPT_TPIC_TRANSPARENT_FILL: i32 = 1 << 1;
pub const OPT_CIDFONT_FIXEDPITCH: i32 = 1 << 2;
pub const OPT_FONTMAP_FIRST_MATCH: i32 = 1 << 3;
pub const OPT_PDFDOC_NO_DEST_REMOVE: i32 = 1 << 4;
pub const OPT_PDFOBJ_NO_PREDICTOR: i32 = 1 << 5;
pub const OPT_PDFOBJ_NO_OBJSTM: i32 = 1 << 6;

/// A `long_options` entry: name, has an argument, the code returned.
pub const LONG_OPTIONS: [(&[u8], bool, i32); 7] = [
    (b"help", false, b'h' as i32),
    (b"version", false, 130),
    (b"showpaper", false, 131),
    (b"dvipdfm", false, 132),
    (b"mvorigin", false, 1000),
    (b"kpathsea-debug", true, 133),
    (b"pdfm-str-utf8", false, 134),
];

/// `DEFAULT_PAPER_NAME` (dpxconf.h).
pub const DEFAULT_PAPER_NAME: &[u8] = b"a4";

/// `struct paper` (dpxconf.h without libpaper).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Paper {
    pub name: &'static [u8],
    pub pswidth: f64,
    pub psheight: f64,
}

/// `paperspecs` (dpxconf.c, without `USE_ISO_PAPERSIZE`; C's NULL
/// terminator dropped).
///
/// TeX Live builds dvipdfm-x with `HAVE_LIBPAPER` (configure.ac), so the
/// real lookup is libpaper's table (not in this tree): this one must be
/// checked against libpaper's `paperspecs.h` before relying on it.
pub const PAPERSPECS: &[Paper] = &[
    Paper {
        name: b"letter",
        pswidth: 612.00,
        psheight: 792.00,
    },
    Paper {
        name: b"legal",
        pswidth: 612.00,
        psheight: 1008.00,
    },
    Paper {
        name: b"ledger",
        pswidth: 1224.00,
        psheight: 792.00,
    },
    Paper {
        name: b"tabloid",
        pswidth: 792.00,
        psheight: 1224.00,
    },
    Paper {
        name: b"a6",
        pswidth: 297.638,
        psheight: 419.528,
    },
    Paper {
        name: b"a5",
        pswidth: 419.528,
        psheight: 595.276,
    },
    Paper {
        name: b"a4",
        pswidth: 595.276,
        psheight: 841.890,
    },
    Paper {
        name: b"a3",
        pswidth: 841.890,
        psheight: 1190.550,
    },
    Paper {
        name: b"b6",
        pswidth: 364.25,
        psheight: 515.91,
    },
    Paper {
        name: b"b5",
        pswidth: 515.91,
        psheight: 728.50,
    },
    Paper {
        name: b"b4",
        pswidth: 728.50,
        psheight: 1031.81,
    },
    Paper {
        name: b"b3",
        pswidth: 1031.81,
        psheight: 1457.00,
    },
    Paper {
        name: b"b5var",
        pswidth: 515.91,
        psheight: 651.97,
    },
    Paper {
        name: b"jisb6",
        pswidth: 364.25,
        psheight: 515.91,
    },
    Paper {
        name: b"jisb5",
        pswidth: 515.91,
        psheight: 728.50,
    },
    Paper {
        name: b"jisb4",
        pswidth: 728.50,
        psheight: 1031.81,
    },
    Paper {
        name: b"jisb3",
        pswidth: 1031.81,
        psheight: 1457.00,
    },
    Paper {
        name: b"isob6",
        pswidth: 354.331,
        psheight: 498.898,
    },
    Paper {
        name: b"isob5",
        pswidth: 498.898,
        psheight: 708.661,
    },
    Paper {
        name: b"isob4",
        pswidth: 708.661,
        psheight: 1000.630,
    },
    Paper {
        name: b"isob3",
        pswidth: 1000.630,
        psheight: 1417.320,
    },
];

/// `paperinfo`: the paper named `ppformat`.
#[must_use]
pub fn paperinfo(ppformat: &[u8]) -> Option<&'static Paper> {
    todo!()
}

/// `struct page_range`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PageRange {
    pub first: i32,
    pub last: i32,
}

/// dvipdfmx.c's statics and globals.
#[derive(Clone, Debug)]
pub struct State {
    /// `my_name` (`xdvipdfmx` for us).
    pub my_name: Vec<u8>,
    pub verbose: i32,
    pub really_quiet: i32,
    /// `OPT_*` bits.
    pub opt_flags: i32,
    pub pdf_version_major: i32,
    pub pdf_version_minor: i32,
    pub compression_level: i32,
    pub annot_grow_x: f64,
    pub annot_grow_y: f64,
    pub bookmark_open: i32,
    pub mag: f64,
    pub enable_thumbnail: i32,
    pub font_dpi: i32,
    pub pdfdecimaldigits: i32,
    pub ignore_colors: i32,
    /// Hours; -2 ignores the image cache (default).
    pub image_cache_life: i32,
    /// Image format conversion filter template.
    pub filter_template: Option<Vec<u8>>,
    pub do_encryption: i32,
    pub key_bits: i32,
    pub permission: i32,
    /// Global (dvi.h): the paper size.
    pub paper_width: f64,
    pub paper_height: f64,
    pub x_offset: f64,
    pub y_offset: f64,
    /// Global (dvi.h).
    pub landscape_mode: i32,
    /// Global (dvi.h).
    pub dvi_ptex_with_vert: i32,
    pub translate_origin: i32,
    pub has_paper_option: i32,
    /// `page_ranges` (`num_page_ranges` is the length).
    pub page_ranges: Vec<PageRange>,
    /// libpaper's `systempapername()` (`PAPERSIZE`, `/etc/papersize`),
    /// given by the host; none: `DEFAULT_PAPER_NAME`.
    pub system_paper_name: Option<Vec<u8>>,
}

impl Default for State {
    fn default() -> Self {
        State {
            my_name: b"xdvipdfmx".to_vec(),
            verbose: 0,
            really_quiet: 0,
            opt_flags: 0,
            pdf_version_major: 1,
            pdf_version_minor: 5,
            compression_level: 9,
            annot_grow_x: 0.0,
            annot_grow_y: 0.0,
            bookmark_open: 0,
            mag: 1.0,
            enable_thumbnail: 0,
            font_dpi: 600,
            pdfdecimaldigits: 3,
            ignore_colors: 0,
            image_cache_life: -2,
            filter_template: None,
            do_encryption: 0,
            key_bits: 40,
            permission: 0x003C,
            paper_width: 595.0,
            paper_height: 842.0,
            x_offset: 72.0,
            y_offset: 72.0,
            landscape_mode: 0,
            dvi_ptex_with_vert: 0,
            translate_origin: 0,
            has_paper_option: 0,
            page_ranges: Vec::new(),
            system_paper_name: None,
        }
    }
}

impl Dpx {
    /// `set_default_pdf_filename`: from `dvi_filename`.
    fn set_default_pdf_filename(&mut self) {
        todo!()
    }
    /// `select_paper`.
    fn select_paper(&mut self, paperspec: &[u8]) {
        todo!()
    }
    /// `select_pages`.
    fn select_pages(&mut self, pagespec: &[u8]) {
        todo!()
    }
    /// `compute_id_string`: the MD5 of the date (no timezone), producer,
    /// DVI and PDF names (`partex_engine::md5::md5` of the concatenation).
    pub fn compute_id_string(
        &mut self,
        producer: Option<&[u8]>,
        dviname: Option<&[u8]>,
        pdfname: Option<&[u8]>,
    ) -> [u8; 16] {
        todo!()
    }
    /// `do_args_first_pass`: `args[0]` is the program name (getopt's argv).
    fn do_args_first_pass(&mut self, args: &[Vec<u8>], source: Option<&[u8]>, unsafe_: i32) {
        todo!()
    }
    /// `do_args_second_pass`.
    fn do_args_second_pass(&mut self, args: &[Vec<u8>], source: Option<&[u8]>, unsafe_: i32) {
        todo!()
    }
    /// `cleanup`.
    fn cleanup(&mut self) {
        todo!()
    }
    /// `read_config_file`: each line `option [value]` through
    /// `do_args_second_pass`.
    pub fn read_config_file(&mut self, config: &[u8]) {
        todo!()
    }
    /// `read_config_special` (`dvipdfmx:config`): one option from
    /// `s[*pp..]`, unsafe.
    pub fn read_config_special(&mut self, s: &[u8], pp: &mut usize) {
        todo!()
    }
    /// `system_default`: the system paper size.
    fn system_default(&mut self) {
        todo!()
    }
    /// `do_dvi_pages`.
    pub fn do_dvi_pages(&mut self) {
        todo!()
    }
    /// `main` (as `xdvipdfmx`, XDV in): `args` is argv; 0 on success.
    pub fn dvipdfmx_main(&mut self, args: &[Vec<u8>]) -> i32 {
        todo!()
    }
}
