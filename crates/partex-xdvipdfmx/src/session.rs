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

/// `paperinfo`: the paper named `ppformat`, as libpaper 2 (TeX Live's
/// xdvipdfmx links the system's) gives it: ISO sizes from millimetres
/// (A4 is 841.8897637795276bp high, not dpxconf's 841.89).
#[must_use]
pub fn paperinfo(ppformat: &[u8]) -> Option<Paper> {
    let mm = |w: f64, h: f64| (w * 72.0 / 25.4, h * 72.0 / 25.4);
    let lower: Vec<u8> = ppformat.iter().map(u8::to_ascii_lowercase).collect();
    let (w, h) = match &lower[..] {
        b"a3" => mm(297.0, 420.0),
        b"a4" => mm(210.0, 297.0),
        b"a5" => mm(148.0, 210.0),
        b"a6" => mm(105.0, 148.0),
        b"b4" => mm(250.0, 353.0),
        b"b5" => mm(176.0, 250.0),
        b"letter" => (612.0, 792.0),
        b"legal" => (612.0, 1008.0),
        b"ledger" => (1224.0, 792.0),
        b"tabloid" => (792.0, 1224.0),
        _ => {
            let p = PAPERSPECS.iter().find(|p| p.name == &lower[..])?;
            (p.pswidth, p.psheight)
        }
    };
    let name = PAPERSPECS
        .iter()
        .find(|p| p.name == &lower[..])
        .map_or(&b"custom"[..], |p| p.name);
    Some(Paper {
        name,
        pswidth: w,
        psheight: h,
    })
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
    /// `SOURCE_DATE_EPOCH` (C reads the environment).
    pub source_date_epoch: Option<i64>,
    /// The time, without `SOURCE_DATE_EPOCH` (C's `time()`), and the
    /// local zone's offset from UTC in minutes (`localtime`).
    pub now: i64,
    pub utc_offset_min: i32,
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
            source_date_epoch: None,
            now: 0,
            utc_offset_min: 0,
        }
    }
}

impl Dpx {
    /// `set_default_pdf_filename`: from `dvi_filename`.
    pub(crate) fn session_set_default_pdf_filename(&mut self) {
        let Some(dvi) = self.dvi_filename.clone() else {
            return;
        };
        let base = dvi.rsplit(|&c| c == b'/').next().unwrap_or(&dvi).to_vec();
        let lower: Vec<u8> = base.iter().map(u8::to_ascii_lowercase).collect();
        let stem = if lower.len() > 4 && (lower.ends_with(b".dvi") || lower.ends_with(b".xdv")) {
            &base[..base.len() - 4]
        } else {
            &base[..]
        };
        let mut p = stem.to_vec();
        p.extend_from_slice(b".pdf");
        self.pdf_filename = Some(p);
    }
    /// `select_paper`.
    fn select_paper(&mut self, paperspec: &[u8]) {
        if let Some(pi) = paperinfo(paperspec) {
            self.session.paper_width = pi.pswidth;
            self.session.paper_height = pi.psheight;
        } else {
            let Some(comma) = paperspec.iter().position(|&c| c == b',') else {
                crate::error!("Unrecognized paper format");
            };
            let mut p = 0;
            let (_, w) = crate::dpxutil::dpx_util_read_length(1.0, &paperspec[..comma], &mut p);
            let mut p = 0;
            let (error, h) =
                crate::dpxutil::dpx_util_read_length(1.0, &paperspec[comma + 1..], &mut p);
            self.session.paper_width = w;
            self.session.paper_height = h;
            if error != 0 || w <= 0.0 || h <= 0.0 {
                crate::error!("Invalid paper size");
            }
        }
    }
    /// `select_pages` (`-s`; xelatex never passes it).
    fn select_pages(&mut self, _pagespec: &[u8]) {
        crate::error!("-s (page selection) is not supported");
    }
    /// `compute_id_string`: the MD5 of the date (no timezone), producer,
    /// DVI and PDF names.
    pub fn compute_id_string(
        &mut self,
        producer: Option<&[u8]>,
        dviname: Option<&[u8]>,
        pdfname: Option<&[u8]>,
    ) -> [u8; 16] {
        let mut data = self.dpx_util_format_asn_date(false);
        for s in [producer, dviname, pdfname].into_iter().flatten() {
            data.extend_from_slice(s);
        }
        partex_engine::md5::md5(&data)
    }
    /// The `PRODUCER` string (`"%s-%s, Copyright ..."`).
    #[must_use]
    pub fn producer_string(&self) -> Vec<u8> {
        let mut p = self.session.my_name.clone();
        p.push(b'-');
        p.extend_from_slice(VERSION);
        p.extend_from_slice(PRODUCER_TAIL);
        p
    }
    /// `do_args_first_pass`: `args[0]` is the program name (getopt's argv).
    fn do_args_first_pass_(&mut self, args: &[Vec<u8>], _source: Option<&[u8]>, _unsafe: i32) {
        for (c, optarg) in getopt(args) {
            match c {
                132 => self.conf.compat_mode = crate::ctx::CompatMode::Compat,
                134 => self.conf.pdfm_str_utf8 = true,
                1000 => self.session.translate_origin = 1,
                0x71 /* q */ => self.session.really_quiet = 2,
                0x76 /* v */ => self.session.verbose += 1,
                0x4d /* M */ => self.conf.compat_mode = crate::ctx::CompatMode::Mpost,
                0x6d /* m */ => {
                    let a = optarg.unwrap_or_default();
                    let (v, n) = crate::fmt::strtod(&a);
                    if v < 0.0 || n == 0 {
                        crate::error!("Invalid magnification specified");
                    }
                    self.session.mag = v;
                }
                _ => {}
            }
        }
    }
    /// `do_args_second_pass`.
    pub(crate) fn do_args_second_pass(
        &mut self,
        args: &[Vec<u8>],
        _source: Option<&[u8]>,
        unsafe_: i32,
    ) {
        use crate::dpxutil::dpx_util_read_length;
        for (c, optarg) in getopt(args) {
            let a = optarg.unwrap_or_default();
            match c as u8 {
                b'D' if c < 128 => {
                    if unsafe_ == 0 {
                        self.session.filter_template = Some(a);
                    }
                }
                b'r' if c < 128 => {
                    self.session.font_dpi = crate::fmt::atoi(&a) as i32;
                    if self.session.font_dpi <= 0 {
                        crate::error!("Invalid bitmap font dpi specified");
                    }
                }
                b'g' if c < 128 => {
                    if let Some(comma) = a.iter().position(|&ch| ch == b',') {
                        let mut p = 0;
                        let (e, x) = dpx_util_read_length(1.0, &a[..comma], &mut p);
                        self.session.annot_grow_x = x;
                        if e == 0 {
                            let mut p = 0;
                            let (_, y) = dpx_util_read_length(1.0, &a[comma + 1..], &mut p);
                            self.session.annot_grow_y = y;
                        }
                    } else {
                        let mut p = 0;
                        let (e, x) = dpx_util_read_length(1.0, &a, &mut p);
                        self.session.annot_grow_x = x;
                        if e == 0 {
                            self.session.annot_grow_y = x;
                        }
                    }
                }
                b'x' if c < 128 => {
                    let mut p = 0;
                    self.session.x_offset = dpx_util_read_length(1.0, &a, &mut p).1;
                }
                b'y' if c < 128 => {
                    let mut p = 0;
                    self.session.y_offset = dpx_util_read_length(1.0, &a, &mut p).1;
                }
                b'o' if c < 128 => self.pdf_filename = Some(a),
                b's' if c < 128 => self.select_pages(&a),
                b't' if c < 128 => self.session.enable_thumbnail = 1,
                b'p' if c < 128 => {
                    self.select_paper(&a);
                    self.session.has_paper_option = 1;
                }
                b'c' if c < 128 => self.session.ignore_colors = 1,
                b'l' if c < 128 => self.session.landscape_mode = 1,
                b'f' if c < 128 => {
                    let mode = if self.session.opt_flags & OPT_FONTMAP_FIRST_MATCH != 0 {
                        crate::fontmap::FONTMAP_RMODE_APPEND
                    } else {
                        crate::fontmap::FONTMAP_RMODE_REPLACE
                    };
                    self.pdf_load_fontmap_file(&a, mode);
                }
                b'i' if c < 128 => {
                    if !a.contains(&b'/') && !a.contains(&b'\\') {
                        self.read_config_file(&a);
                    }
                }
                b'V' if c < 128 => {
                    if a.contains(&b'.') {
                        let tmp = crate::fmt::atof(&a);
                        self.session.pdf_version_major = tmp as i32;
                        self.session.pdf_version_minor =
                            (10.0 * (tmp - f64::from(self.session.pdf_version_major)) + 0.5) as i32;
                    } else {
                        self.session.pdf_version_major = 1;
                        self.session.pdf_version_minor = crate::fmt::atoi(&a) as i32;
                    }
                }
                b'z' if c < 128 => self.session.compression_level = crate::fmt::atoi(&a) as i32,
                b'd' if c < 128 => self.session.pdfdecimaldigits = crate::fmt::atoi(&a) as i32,
                b'I' if c < 128 => self.session.image_cache_life = crate::fmt::atoi(&a) as i32,
                b'S' if c < 128 => self.session.do_encryption = 1,
                b'K' if c < 128 => self.session.key_bits = crate::fmt::atoi(&a) as i32,
                b'P' if c < 128 => {
                    let (v, n) = crate::fmt::strtol(&a, 0);
                    if n == 0 {
                        crate::error!("Invalid encryption permission flag");
                    }
                    self.session.permission = v as u32 as i32;
                }
                b'O' if c < 128 => self.session.bookmark_open = crate::fmt::atoi(&a) as i32,
                b'C' if c < 128 => {
                    let (v, n) = crate::fmt::strtol(&a, 0);
                    if n == 0 {
                        crate::error!("Invalid flag");
                    }
                    let flags = v as u32 as i32;
                    if flags < 0 {
                        self.session.opt_flags = -flags;
                    } else {
                        self.session.opt_flags |= flags;
                    }
                }
                b'E' if c < 128 => self.conf.ignore_font_license = true,
                _ => {}
            }
        }
    }
    /// `cleanup`.
    fn cleanup(&mut self) {}
    /// `read_config_file`: each line `option [value]` through
    /// `do_args_second_pass`.
    pub fn read_config_file(&mut self, config: &[u8]) {
        let Some(mut fp) = self.dpx_open_file(config, crate::dpxfile::ResType::Text) else {
            return;
        };
        while let Some(line) = fp.mfgets(1024) {
            let mut p = 0;
            crate::parse::skip_white(&line, &mut p);
            if p >= line.len() {
                continue;
            }
            let mut argv: Vec<Vec<u8>> = vec![b"config_file".to_vec()];
            if let Some(option) = crate::parse::parse_ident(&line, &mut p) {
                let mut o = vec![b'-'];
                o.extend_from_slice(&option);
                argv.push(o);
                crate::parse::skip_white(&line, &mut p);
                if p < line.len() {
                    let v = if line[p] == b'"' {
                        crate::dpxutil::parse_c_string(&line, &mut p)
                    } else {
                        crate::parse::parse_ident(&line, &mut p)
                    };
                    argv.push(v.unwrap_or_default());
                }
            }
            self.do_args_second_pass(&argv, Some(config), 0);
        }
    }
    /// `read_config_special` (`dvipdfmx:config`): one option from
    /// `s[*pp..]`, unsafe.
    pub fn read_config_special(&mut self, s: &[u8], pp: &mut usize) {
        crate::parse::skip_white(s, pp);
        if *pp >= s.len() {
            return;
        }
        let mut argv: Vec<Vec<u8>> = vec![b"config_special".to_vec()];
        if let Some(option) = crate::parse::parse_ident(s, pp) {
            let mut o = vec![b'-'];
            o.extend_from_slice(&option);
            argv.push(o);
            crate::parse::skip_white(s, pp);
            if *pp < s.len() {
                let v = if s[*pp] == b'"' {
                    crate::dpxutil::parse_c_string(s, pp)
                } else {
                    crate::parse::parse_ident(s, pp)
                };
                argv.push(v.unwrap_or_default());
            }
        }
        self.do_args_second_pass(&argv, Some(b"config_special"), 1);
    }
    /// `system_default`: the system paper size.
    pub(crate) fn session_system_default(&mut self) {
        let name = self
            .session
            .system_paper_name
            .clone()
            .unwrap_or_else(|| DEFAULT_PAPER_NAME.to_vec());
        self.select_paper(&name);
    }
    /// `do_dvi_pages`: the whole file, linear (C's loop; the page API in
    /// `crate::api` does the same a page at a time).
    pub fn do_dvi_pages(&mut self) {
        crate::error!("do_dvi_pages: use crate::api::Session");
    }
    /// `main` (as `xdvipdfmx`, XDV in): `args` is argv; 0 on success.
    pub fn dvipdfmx_main(&mut self, _args: &[Vec<u8>]) -> i32 {
        crate::error!("dvipdfmx_main: use crate::api::Session");
    }
}

/// getopt_long over `args` (`args[0]` the program name) with `OPTSTRIG`
/// and `LONG_OPTIONS`: each option's code and argument, in order.
fn getopt(args: &[Vec<u8>]) -> Vec<(i32, Option<Vec<u8>>)> {
    let mut out = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a.len() < 2 || a[0] != b'-' {
            // (a non-option: the DVI name, or a config line's stray word)
            i += 1;
            continue;
        }
        if a.starts_with(b"--") {
            let name = &a[2..];
            if let Some(&(_, has_arg, code)) = LONG_OPTIONS.iter().find(|(n, _, _)| *n == name) {
                let v = if has_arg {
                    i += 1;
                    args.get(i).cloned()
                } else {
                    None
                };
                out.push((code, v));
            }
            i += 1;
            continue;
        }
        let mut k = 1;
        while k < a.len() {
            let c = a[k];
            let pos = OPTSTRIG.iter().position(|&o| o == c && o != b':');
            let takes_arg = pos.is_some_and(|p| OPTSTRIG.get(p + 1) == Some(&b':'));
            if takes_arg {
                let v = if k + 1 < a.len() {
                    Some(a[k + 1..].to_vec())
                } else {
                    i += 1;
                    args.get(i).cloned()
                };
                out.push((i32::from(c), v));
                break;
            }
            out.push((i32::from(c), None));
            k += 1;
        }
        i += 1;
    }
    out
}

impl Dpx {
    /// `do_args_first_pass` over xdvipdfmx's argv.
    pub(crate) fn session_args_first_pass(&mut self, args: &[Vec<u8>]) {
        self.do_args_first_pass_(args, None, 0);
        // (the DVI name C takes from argv is `Options::dvi_filename`)
    }
    /// `do_args_second_pass` over xdvipdfmx's argv.
    pub(crate) fn session_args_second_pass(&mut self, args: &[Vec<u8>]) {
        self.do_args_second_pass(args, None, 0);
    }
}
