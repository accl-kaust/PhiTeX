//! xdvipdfmx a page at a time: dvipdfmx.c's `main` and `do_dvi_pages`,
//! cut where the engine can make each page a node.
//!
//! [`Session::new`] takes the options and the XDV's preamble; each
//! [`Session::page`] takes one page's bytes (from after the previous
//! `eop` through this page's `eop`, font definitions included) and
//! returns the PDF bytes written meanwhile and the page's glyph runs;
//! [`Session::finish`] writes the rest (fonts, the document's objects,
//! the object streams, the xref stream, the trailer). The bytes of all
//! the calls, concatenated, are xdvipdfmx's PDF.
//!
//! A fatal error (C's `ERROR`, which prints its message and exits) is an
//! `Err(`[`Fatal`]`)`: the session is then done, and [`Session::written`]
//! gives the rest of what xdvipdfmx had written to its output file when
//! it exited (`error_cleanup` leaves the file). A malformed `\special`,
//! a broken image or font file stop the run that way, never by a panic.
//!
//! What a page's bytes depend on (for splicing pages): the first page
//! also writes the header (`pdf_open_document`, after the first page's
//! specials chose the version and the paper). A page's content stream is
//! written when the page ends; everything that goes into object streams
//! (resources dicts, annotations, …) is written when an object stream
//! fills (200 objects), which can be during any later page or at the end,
//! so object numbers and object-stream boundaries depend on the earlier
//! pages, and the page dictionaries and the page tree are only written at
//! the end. Fonts get their numbers at first use (`pdf_get_font_reference`)
//! and their subset tags in the order of the end call's font loop, which
//! is the order of first use.

use alloc::sync::Arc;

use crate::dvi::{GlyphRun, ScanSpecials, ScanSpecialsExt};
use crate::io::Files;
use crate::obj::Deflate;
use crate::pdfdev::PdfRect;
use crate::pdfdoc::{PdfDevSetting, PdfEncSetting, PdfObjSetting, PdfSetting};
use crate::prelude::*;

/// How xdvipdfmx is run.
#[derive(Clone, Debug)]
pub struct Options {
    /// `-o`: the PDF's name (hashed into the subset tags and the ID).
    pub pdf_filename: Option<Vec<u8>>,
    /// The DVI file's name (none: the XDV is piped in, as xelatex does).
    pub dvi_filename: Option<Vec<u8>>,
    /// The other command-line options (xelatex: `-q -E`).
    pub args: Vec<Vec<u8>>,
    /// `SOURCE_DATE_EPOCH`.
    pub source_date_epoch: Option<i64>,
    /// Without it, the time and the zone's offset (minutes) from UTC.
    pub now: i64,
    pub utc_offset_min: i32,
    /// libpaper's system paper name (none: `a4`; `dvipdfmx.cfg` sets
    /// `p a4` anyway).
    pub system_paper: Option<Vec<u8>>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            pdf_filename: None,
            dvi_filename: None,
            args: vec![b"-q".to_vec(), b"-E".to_vec()],
            source_date_epoch: None,
            now: 0,
            utc_offset_min: 0,
            system_paper: None,
        }
    }
}

/// What a page wrote.
#[derive(Clone, Debug, Default)]
pub struct PageOut {
    /// The PDF bytes written during the call.
    pub pdf: Vec<u8>,
    /// The page's glyphs (native fonts).
    pub glyph_runs: Vec<GlyphRun>,
}

/// A conversion in progress.
/// A conversion in progress.
pub struct Session {
    dpx: Box<Dpx>,
    argv: Vec<Vec<u8>>,
    /// The next page's number (0-based).
    page_no: i32,
    page_count: i32,
    init_paper_width: f64,
    init_paper_height: f64,
}

impl Session {
    /// dvipdfmx.c's `main` up to `dvi_init` and the comment: the options,
    /// the config file, the preamble. Nothing is written yet, so a fatal
    /// error here leaves no output.
    pub fn new(
        options: Options,
        files: Box<dyn Files>,
        deflate: Deflate,
        preamble: &[u8],
    ) -> Result<Self> {
        let mut dpx = Box::new(Dpx::new(files, deflate));
        dpx.session.source_date_epoch = options.source_date_epoch;
        dpx.session.now = options.now;
        dpx.session.utc_offset_min = options.utc_offset_min;
        dpx.session.system_paper_name = options.system_paper.clone();
        let mut argv = vec![b"xdvipdfmx".to_vec()];
        argv.extend(options.args.iter().cloned());
        if let Some(p) = &options.pdf_filename {
            argv.push(b"-o".to_vec());
            argv.push(p.clone());
        }
        // (C takes the DVI name from argv; dvi_init would add `.xdv` to
        // a name without a suffix: give it with one)
        dpx.dvi_filename = options.dvi_filename.clone();
        dpx.session_args_first_pass(&argv)?;
        dpx.session_system_default()?;
        dpx.pdf_init_fontmaps();
        dpx.read_config_file(crate::session::DPX_CONFIG_FILE)?;
        dpx.session.has_paper_option = 0;
        if dpx.dvi_filename.is_some() && dpx.pdf_filename.is_none() {
            dpx.session_set_default_pdf_filename();
        }
        dpx.dvi.dvi_file = Some(MemFile::new(Arc::from(preamble), b"stdin"));
        dpx.dvi.peek_after_eop = false;
        let mag = dpx.session.mag;
        let dvi2pts = dpx.dvi_init(None, mag)?;
        if dvi2pts == 0.0 {
            crate::fatal!("dvi_init() failed!");
        }
        Ok(Session {
            dpx,
            argv,
            page_no: 0,
            page_count: 0,
            init_paper_width: 0.0,
            init_paper_height: 0.0,
        })
    }

    /// The rest of `main` once the first page is in: its specials, the
    /// options again, `pdf_open_document`, `do_dvi_pages`' start.
    fn open(&mut self) -> Result<()> {
        let d = &mut *self.dpx;
        let creator = d.dvi_comment();
        let mut sp = ScanSpecials {
            page_width: d.session.paper_width,
            page_height: d.session.paper_height,
            x_offset: d.session.x_offset,
            y_offset: d.session.y_offset,
            landscape: d.session.landscape_mode,
            ext: Some(ScanSpecialsExt {
                majorversion: d.session.pdf_version_major,
                minorversion: d.session.pdf_version_minor,
                do_enc: d.session.do_encryption,
                key_bits: d.session.key_bits,
                permission: d.session.permission,
                ..ScanSpecialsExt::default()
            }),
        };
        d.dvi_scan_specials(0, &mut sp)?;
        d.session.paper_width = sp.page_width;
        d.session.paper_height = sp.page_height;
        d.session.x_offset = sp.x_offset;
        d.session.y_offset = sp.y_offset;
        d.session.landscape_mode = sp.landscape;
        let ext = sp.ext.take().expect("ext");
        d.session.pdf_version_major = ext.majorversion;
        d.session.pdf_version_minor = ext.minorversion;
        d.session.do_encryption = ext.do_enc;
        d.session.key_bits = ext.key_bits;
        d.session.permission = ext.permission;
        if d.session.do_encryption != 0 {
            crate::fatal!("Encryption is not supported");
        }
        let argv = self.argv.clone();
        d.session_args_second_pass(&argv)?;
        if d.pdf_filename.as_deref() == Some(b"-") {
            d.pdf_filename = None;
        }
        let font_dpi = d.session.font_dpi;
        d.pdf_font_set_dpi(font_dpi);
        let (id1, id2) = if ext.has_id != 0 {
            (ext.id1, ext.id2)
        } else {
            let producer = d.producer_string();
            let (dvi, pdf) = (d.dvi_filename.clone(), d.pdf_filename.clone());
            let id = d.compute_id_string(Some(&producer), dvi.as_deref(), pdf.as_deref());
            (id, id)
        };
        if d.session.landscape_mode != 0 {
            core::mem::swap(&mut d.session.paper_width, &mut d.session.paper_height);
        }
        let s = &d.session;
        let settings = PdfSetting {
            ver_major: s.pdf_version_major,
            ver_minor: s.pdf_version_minor,
            media_width: s.paper_width,
            media_height: s.paper_height,
            annot_grow_amount: (s.annot_grow_x, s.annot_grow_y),
            outline_open_depth: s.bookmark_open,
            check_gotos: i32::from(s.opt_flags & crate::session::OPT_PDFDOC_NO_DEST_REMOVE == 0),
            enable_manual_thumb: s.enable_thumbnail,
            enable_encrypt: 0,
            encrypt: PdfEncSetting::default(),
            device: PdfDevSetting {
                dvi2pts: d.dvi.dvi2pts,
                precision: s.pdfdecimaldigits,
                ignore_colors: s.ignore_colors,
            },
            object: PdfObjSetting {
                enable_objstm: i32::from(s.opt_flags & crate::session::OPT_PDFOBJ_NO_OBJSTM == 0),
                enable_predictor: i32::from(
                    s.opt_flags & crate::session::OPT_PDFOBJ_NO_PREDICTOR == 0,
                ),
                compression_level: s.compression_level,
            },
        };
        let pdf_filename = d.pdf_filename.clone();
        d.pdf_open_document(
            pdf_filename.as_deref(),
            Some(&creator),
            &id1,
            &id2,
            settings,
        )?;
        if d.session.opt_flags & crate::session::OPT_CIDFONT_FIXEDPITCH != 0 {
            d.CIDFont_set_flags(crate::cid::CIDFONT_FORCE_FIXEDPITCH);
        }
        if d.session.opt_flags & crate::session::OPT_TPIC_TRANSPARENT_FILL != 0
            || d.session.translate_origin != 0
        {
            crate::fatal!("tpic and MetaPost options are not supported");
        }
        // do_dvi_pages
        d.spc_exec_at_begin_document()?;
        self.init_paper_width = d.session.paper_width;
        self.init_paper_height = d.session.paper_height;
        let mediabox = PdfRect {
            llx: 0.0,
            lly: 0.0,
            urx: d.session.paper_width,
            ury: d.session.paper_height,
        };
        d.pdf_doc_set_mediabox(0, &mediabox);
        Ok(())
    }

    /// One page: `bytes` from after the previous page's `eop` (or the
    /// preamble) through this page's `eop`. After an `Err`, only
    /// [`Session::written`] is left to call.
    pub fn page(&mut self, bytes: &[u8]) -> Result<PageOut> {
        let first = self.page_no == 0;
        self.dpx.dvi.dvi_file = Some(MemFile::new(Arc::from(bytes), b"stdin"));
        if first {
            self.open()?;
        }
        let d = &mut *self.dpx;
        let page_no = self.page_no;
        let (mut page_width, mut page_height) = (d.session.paper_width, d.session.paper_height);
        let mut sp = ScanSpecials {
            page_width,
            page_height,
            x_offset: d.session.x_offset,
            y_offset: d.session.y_offset,
            landscape: d.session.landscape_mode,
            ext: None,
        };
        d.dvi_scan_specials(page_no, &mut sp)?;
        let (mut w, mut h) = (sp.page_width, sp.page_height);
        if sp.landscape != d.session.landscape_mode {
            core::mem::swap(&mut w, &mut h);
            d.session.landscape_mode = sp.landscape;
        }
        if d.session.has_paper_option == 0 && (page_width != w || page_height != h) {
            page_width = w;
            page_height = h;
        }
        if d.session.x_offset != sp.x_offset || d.session.y_offset != sp.y_offset {
            d.session.x_offset = sp.x_offset;
            d.session.y_offset = sp.y_offset;
        }
        if page_width != self.init_paper_width || page_height != self.init_paper_height {
            let mediabox = PdfRect {
                llx: 0.0,
                lly: 0.0,
                urx: page_width,
                ury: page_height,
            };
            d.pdf_doc_set_mediabox((self.page_count + 1) as u32, &mediabox);
        }
        d.dvi.glyph_runs.clear();
        let (xo, yo) = (d.session.x_offset, d.session.y_offset);
        d.dvi_do_page(page_height, xo, yo)?;
        self.page_count += 1;
        self.page_no += 1;
        Ok(PageOut {
            pdf: d.o.take_output(),
            glyph_runs: core::mem::take(&mut d.dvi.glyph_runs),
        })
    }

    /// The end: `do_dvi_pages`' end, `pdf_close_document` and the rest of
    /// `main`. The bytes written. After it (`Ok` or `Err`), only
    /// [`Session::written`] is left to call.
    pub fn finish(&mut self) -> Result<Vec<u8>> {
        if self.page_count < 1 {
            crate::fatal!("No pages fall in range!");
        }
        let d = &mut *self.dpx;
        d.spc_exec_at_end_document()?;
        d.pdf_close_document()?;
        d.pdf_close_fontmaps();
        d.dvi_close();
        Ok(d.o.take_output())
    }

    /// What was written but not yet taken: after an `Err`, the output
    /// file's rest as the fatal error left it (`pdf_error_cleanup` closes
    /// the file, which `error_cleanup` then does not remove).
    pub fn written(&mut self) -> Vec<u8> {
        self.dpx.o.take_output()
    }

    /// A whole XDV, as [`Session::new`], [`Session::page`] for each page
    /// and [`Session::finish`] do it. (On an `Err`, the bytes written
    /// before it are dropped: [`Session`] keeps them.)
    pub fn convert(
        options: Options,
        files: Box<dyn Files>,
        deflate: Deflate,
        xdv: &[u8],
    ) -> Result<Vec<u8>> {
        let (pre, pages) = split_xdv(xdv);
        let mut s = Session::new(options, files, deflate, &xdv[..pre])?;
        let mut out = Vec::new();
        for (a, b) in pages {
            out.extend(s.page(&xdv[a..b])?.pdf);
        }
        out.extend(s.finish()?);
        Ok(out)
    }
}

/// An XDV's preamble's end and each page's span (from after the previous
/// page through its `eop`); the postamble is left out.
#[must_use]
pub fn split_xdv(x: &[u8]) -> (usize, Vec<(usize, usize)>) {
    use crate::dvi::*;
    let be = |p: usize, n: usize| -> u32 {
        x[p..p + n]
            .iter()
            .fold(0u32, |a, &b| (a << 8) | u32::from(b))
    };
    // pre i[1] num[4] den[4] mag[4] k[1] x[k]
    let pre = 15 + x[14] as usize;
    let mut pages = Vec::new();
    let mut p = pre;
    let mut start = pre;
    while p < x.len() {
        let op = x[p];
        p += 1;
        match op {
            0..=127
            | FNT_NUM_0..=FNT_NUM_63
            | NOP
            | PUSH
            | POP
            | W0
            | X0
            | Y0
            | Z0
            | BEGIN_REFLECT
            | END_REFLECT => {}
            SET1..=SET4 => p += (op - SET1 + 1) as usize,
            PUT1..=PUT4 => p += (op - PUT1 + 1) as usize,
            RIGHT1..=RIGHT4 => p += (op - RIGHT1 + 1) as usize,
            W1..=W4 => p += (op - W1 + 1) as usize,
            X1..=X4 => p += (op - X1 + 1) as usize,
            DOWN1..=DOWN4 => p += (op - DOWN1 + 1) as usize,
            Y1..=Y4 => p += (op - Y1 + 1) as usize,
            Z1..=Z4 => p += (op - Z1 + 1) as usize,
            FNT1..=FNT4 => p += (op - FNT1 + 1) as usize,
            SET_RULE | PUT_RULE => p += 8,
            BOP => p += 44,
            EOP => {
                pages.push((start, p));
                start = p;
            }
            XXX1..=XXX4 => {
                let n = (op - XXX1 + 1) as usize;
                let len = be(p, n) as usize;
                p += n + len;
            }
            FNT_DEF1..=FNT_DEF4 => {
                p += (op - FNT_DEF1 + 1) as usize + 12;
                let (a, l) = (x[p] as usize, x[p + 1] as usize);
                p += 2 + a + l;
            }
            XDV_NATIVE_FONT_DEF => {
                p += 4 + 4;
                let flags = be(p, 2) as u16;
                p += 2;
                let len = x[p] as usize;
                p += 1 + len + 4;
                for f in [
                    XDV_FLAG_COLORED,
                    XDV_FLAG_EXTEND,
                    XDV_FLAG_SLANT,
                    XDV_FLAG_EMBOLDEN,
                ] {
                    if flags & f != 0 {
                        p += 4;
                    }
                }
            }
            XDV_GLYPHS => {
                p += 4;
                let n = be(p, 2) as usize;
                p += 2 + n * 10;
            }
            XDV_TEXT_AND_GLYPHS => {
                let n = be(p, 2) as usize;
                p += 2 + n * 2 + 4;
                let n = be(p, 2) as usize;
                p += 2 + n * 10;
            }
            PTEXDIR => p += 1,
            _ => break, // POST
        }
    }
    (pre, pages)
}

impl Session {
    /// A copy of the session as it is (between pages), to go on from
    /// later: the incremental link. A page that changed is done again
    /// from the snapshot taken before it, then the pages after it and the
    /// end (their object numbers follow from the pages before them); the
    /// pages before it keep their bytes. The copy shares only the host's
    /// files and deflater; the fonts' used-glyph tables (shared within a
    /// session, as C shares the pointers) are copied with their sharing.
    #[must_use]
    pub fn snapshot(&self) -> Session {
        let mut dpx = Box::new((*self.dpx).clone());
        let mut map: Vec<(
            *const core::cell::RefCell<Vec<u8>>,
            crate::pdffont::UsedChars,
        )> = Vec::new();
        let mut fresh = |uc: &mut Option<crate::pdffont::UsedChars>| {
            if let Some(old) = uc.take() {
                let p = Rc::as_ptr(&old);
                let new = match map.iter().find(|(q, _)| *q == p) {
                    Some((_, n)) => n.clone(),
                    None => {
                        let n = Rc::new(core::cell::RefCell::new(old.borrow().clone()));
                        map.push((p, n.clone()));
                        n
                    }
                };
                *uc = Some(new);
            }
        };
        for f in &mut dpx.font.fonts {
            fresh(&mut f.usedchars);
            fresh(&mut f.cid.usedchars_v);
        }
        for f in &mut dpx.dev.pdev.fonts {
            fresh(&mut f.used_chars);
        }
        Session {
            dpx,
            argv: self.argv.clone(),
            page_no: self.page_no,
            page_count: self.page_count,
            init_paper_width: self.init_paper_width,
            init_paper_height: self.init_paper_height,
        }
    }

    /// The number of pages done.
    #[must_use]
    pub fn pages_done(&self) -> i32 {
        self.page_no
    }
}

impl Session {
    /// [`Session::written`].
    pub fn take_output(&mut self) -> Vec<u8> {
        self.dpx.o.take_output()
    }
}
