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

pub use crate::direct::Op;
use crate::dvi::{ScanSpecials, ScanSpecialsExt};
pub use crate::glyphrun::{ColorSpec, GlyphRun, GlyphSource};
use crate::io::Files;
pub use crate::obj::BatchDeflate;
use crate::obj::Deflate;
use crate::pdfdev::PdfRect;
use crate::pdfdoc::{PdfDevSetting, PdfEncSetting, PdfObjSetting, PdfSetting};
use crate::prelude::*;

/// How xdvipdfmx is run.
#[derive(Clone, Debug, PartialEq)]
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
    /// Whether each page's [`PageOut::glyph_runs`] are made (a viewer's
    /// side output; the PDF is the same either way).
    pub glyph_runs: bool,
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
            glyph_runs: false,
        }
    }
}

/// What a page wrote.
#[derive(Clone, Debug, Default)]
pub struct PageOut {
    /// The PDF bytes written during the call.
    pub pdf: Vec<u8>,
    /// The page's glyphs, as [`crate::glyphrun`] describes them (none
    /// unless [`Options::glyph_runs`]).
    pub glyph_runs: Vec<GlyphRun>,
}

/// A conversion in progress. It may move between threads (an engine's
/// link keeps one, and its snapshots, from one build to the next, run
/// on whichever thread links): the host's files and deflaters are
/// `Send + Sync`, the fonts' shared tables atomics.
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
        files: Arc<dyn Files>,
        deflate: Deflate,
        preamble: &[u8],
    ) -> Result<Self> {
        let mut dpx = Box::new(Dpx::new(Box::new(NoFiles), deflate));
        dpx.files = crate::ctx::FilesRef(files);
        let argv = Self::start(&mut dpx, &options)?;
        dpx.dvi.dvi_file = Some(MemFile::new(Arc::from(preamble), b"stdin"));
        dpx.dvi.peek_after_eop = false;
        let mag = dpx.session.mag;
        let dvi2pts = dpx.dvi_init(None, mag)?;
        if dvi2pts == 0.0 {
            crate::fatal!("dvi_init() failed!");
        }
        dpx.pdf_fontmaps_started();
        Ok(Session {
            dpx,
            argv,
            page_no: 0,
            page_count: 0,
            init_paper_width: 0.0,
            init_paper_height: 0.0,
        })
    }

    /// A session for pages given as operations ([`Session::page_ops`]),
    /// not as an XDV: [`Session::new`] with XeTeX's preamble (`dvi_mag`,
    /// `comment`) given as it is, not read.
    pub fn direct(
        options: Options,
        files: Arc<dyn Files>,
        deflate: Deflate,
        dvi_mag: u32,
        comment: &[u8],
    ) -> Result<Self> {
        let mut dpx = Box::new(Dpx::new(Box::new(NoFiles), deflate));
        dpx.files = crate::ctx::FilesRef(files);
        let argv = Self::start(&mut dpx, &options)?;
        let mag = dpx.session.mag;
        let dvi2pts = dpx.dvi_init_direct(mag, dvi_mag, comment);
        if dvi2pts == 0.0 {
            crate::fatal!("dvi_init() failed!");
        }
        dpx.pdf_fontmaps_started();
        Ok(Session {
            dpx,
            argv,
            page_no: 0,
            page_count: 0,
            init_paper_width: 0.0,
            init_paper_height: 0.0,
        })
    }

    /// `main` up to `dvi_init`: the options, the config file. The
    /// arguments.
    fn start(dpx: &mut Dpx, options: &Options) -> Result<Vec<Vec<u8>>> {
        dpx.session.source_date_epoch = options.source_date_epoch;
        dpx.session.now = options.now;
        dpx.session.utc_offset_min = options.utc_offset_min;
        dpx.session.system_paper_name = options.system_paper.clone();
        dpx.runs.on = options.glyph_runs;
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
        Ok(argv)
    }

    /// The host's files from now on (a session kept between builds reads
    /// the document's files as they are now).
    pub fn set_files(&mut self, files: Arc<dyn Files>) {
        self.dpx.files = crate::ctx::FilesRef(files);
    }

    /// Page `page_no`'s specials scanned (and its fonts defined), from
    /// the XDV or from its operations.
    fn scan(d: &mut Dpx, page_no: i32, sp: &mut ScanSpecials, ops: Option<&[Op]>) -> Result<()> {
        match ops {
            Some(o) => d.dvi_scan_ops(page_no, sp, o),
            None => d.dvi_scan_specials(page_no, sp),
        }
    }

    /// The rest of `main` once the first page is in: its specials, the
    /// options again, `pdf_open_document`, `do_dvi_pages`' start.
    fn open(&mut self, ops: Option<&[Op]>) -> Result<()> {
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
        Self::scan(d, 0, &mut sp, ops)?;
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
        // (main: the `D` option's distiller, for EPS pictures)
        let filter_template = d.session.filter_template.clone();
        d.set_distiller_template(filter_template.as_deref());
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
        d.pdf_doc_set_mediabox(0, &mediabox)?;
        Ok(())
    }

    /// One page: `bytes` from after the previous page's `eop` (or the
    /// preamble) through this page's `eop`. After an `Err`, only
    /// [`Session::written`] is left to call.
    pub fn page(&mut self, bytes: &[u8]) -> Result<PageOut> {
        self.dpx.dvi.dvi_file = Some(MemFile::new(Arc::from(bytes), b"stdin"));
        self.page_from(None)
    }

    /// One page given as its operations (`bop` and `eop` not among
    /// them), in a session made by [`Session::direct`]: the bytes as
    /// [`Session::page`] gives them for the XDV page those operations
    /// stand for. After an `Err`, only [`Session::written`] is left to
    /// call.
    pub fn page_ops(&mut self, ops: &[Op]) -> Result<PageOut> {
        self.page_from(Some(ops))
    }

    fn page_from(&mut self, ops: Option<&[Op]>) -> Result<PageOut> {
        let first = self.page_no == 0;
        if first {
            self.open(ops)?;
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
        Self::scan(d, page_no, &mut sp, ops)?;
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
            d.pdf_doc_set_mediabox((self.page_count + 1) as u32, &mediabox)?;
        }
        d.runs.page_begin();
        let (xo, yo) = (d.session.x_offset, d.session.y_offset);
        match ops {
            Some(o) => d.dvi_do_ops(o, page_height, xo, yo)?,
            None => d.dvi_do_page(page_height, xo, yo)?,
        }
        self.page_count += 1;
        self.page_no += 1;
        Ok(PageOut {
            pdf: d.o.take_output(),
            glyph_runs: core::mem::take(&mut d.runs.runs),
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
        // (the streams of the end, the fonts' above all, compressed in
        // one batch if the host gave a batch deflater)
        d.o.defer_streams();
        let r = (|| {
            d.spc_exec_at_end_document()?;
            d.pdf_close_document()?;
            d.pdf_close_fontmaps();
            d.dvi_close();
            Ok(())
        })();
        if let Err(e) = r {
            // (what was written, the pending streams in place)
            let _ = d.o.resolve_pending();
            return Err(e);
        }
        Ok(d.o.take_output())
    }

    /// Compresses the streams of [`Session::finish`] (the fonts', the
    /// last object streams) with `batch`, all at once, which the host
    /// may run in parallel; the bytes are the same as with the
    /// session's deflater alone.
    pub fn set_batch_deflate(&mut self, batch: BatchDeflate) {
        self.dpx.o.set_batch_deflate(batch);
    }

    /// The file's cross-reference section as [`Session::finish`] wrote
    /// it: each object's place, the trailer's entries (none before).
    #[must_use]
    pub fn xref_written(&self) -> Option<&crate::obj::XrefWritten> {
        self.dpx.o.xref_written.as_ref()
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
        files: Arc<dyn Files>,
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

/// No files: a session's files until the host's are put in.
struct NoFiles;

impl Files for NoFiles {
    fn find(&self, _: &[u8], _: crate::io::Format, _: &[u8]) -> Option<Vec<u8>> {
        None
    }
    fn read(&self, _: &[u8]) -> Option<Arc<[u8]>> {
        None
    }
}

const _: () = {
    const fn send<T: Send>() {}
    send::<Session>();
};

/// Conversions that start alike, as a watch's builds of a document do:
/// the start [`Session::new`] makes (the options, the config file, the
/// font maps it reads, the preamble) is kept, and each conversion goes on
/// from a copy of it ([`Session::snapshot`]) while the options and the
/// preamble are the same. The copies share the kept start's files and
/// deflater (the host's).
#[derive(Default)]
pub struct Starts {
    kept: Option<(Options, Vec<u8>, Session)>,
}

impl Starts {
    /// A session as `Session::new(options, files, deflate, preamble)`
    /// starts it: a copy of the kept start if it was made with the same
    /// options and preamble (`files` and `deflate` then unused), else a
    /// new start, kept.
    pub fn session(
        &mut self,
        options: Options,
        files: Arc<dyn Files>,
        deflate: Deflate,
        preamble: &[u8],
    ) -> Result<Session> {
        if let Some((o, p, s)) = &self.kept
            && *o == options
            && p.as_slice() == preamble
        {
            return Ok(s.snapshot());
        }
        self.kept = None;
        let s = Session::new(options.clone(), files, deflate, preamble)?;
        let copy = s.snapshot();
        self.kept = Some((options, preamble.to_vec(), s));
        Ok(copy)
    }
}

/// An XDV's preamble's end and each page's span (from after the previous
/// page through its `eop`); the postamble is left out, and so is a page
/// the bytes end in (the driver then stops where C's would: at the
/// preamble's or that page's end).
#[must_use]
pub fn split_xdv(x: &[u8]) -> (usize, Vec<(usize, usize)>) {
    use crate::dvi::*;
    let be = |p: usize, n: usize| -> Option<usize> {
        Some(
            x.get(p..p.checked_add(n)?)?
                .iter()
                .fold(0usize, |a, &b| (a << 8) | usize::from(b)),
        )
    };
    // pre i[1] num[4] den[4] mag[4] k[1] x[k]
    let pre = x
        .get(14)
        .map_or(x.len(), |&k| (15 + usize::from(k)).min(x.len()));
    let mut pages = Vec::new();
    let mut p = pre;
    let mut start = pre;
    // The next opcode's operands skipped: none at the end of the bytes.
    let mut step = |p: &mut usize| -> Option<bool> {
        let op = *x.get(*p)?;
        *p += 1;
        let skip = match op {
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
            | END_REFLECT => 0,
            SET1..=SET4 => usize::from(op - SET1 + 1),
            PUT1..=PUT4 => usize::from(op - PUT1 + 1),
            RIGHT1..=RIGHT4 => usize::from(op - RIGHT1 + 1),
            W1..=W4 => usize::from(op - W1 + 1),
            X1..=X4 => usize::from(op - X1 + 1),
            DOWN1..=DOWN4 => usize::from(op - DOWN1 + 1),
            Y1..=Y4 => usize::from(op - Y1 + 1),
            Z1..=Z4 => usize::from(op - Z1 + 1),
            FNT1..=FNT4 => usize::from(op - FNT1 + 1),
            SET_RULE | PUT_RULE => 8,
            BOP => 44,
            EOP => {
                pages.push((start, *p));
                start = *p;
                0
            }
            XXX1..=XXX4 => {
                let n = usize::from(op - XXX1 + 1);
                n + be(*p, n)?
            }
            FNT_DEF1..=FNT_DEF4 => {
                let n = usize::from(op - FNT_DEF1 + 1) + 12;
                let (a, l) = (be(*p + n, 1)?, be(*p + n + 1, 1)?);
                n + 2 + a + l
            }
            XDV_NATIVE_FONT_DEF => {
                let flags = be(*p + 8, 2)? as u16;
                let len = be(*p + 10, 1)?;
                let extra = [
                    XDV_FLAG_COLORED,
                    XDV_FLAG_EXTEND,
                    XDV_FLAG_SLANT,
                    XDV_FLAG_EMBOLDEN,
                ]
                .iter()
                .filter(|&&f| flags & f != 0)
                .count();
                11 + len + 4 + 4 * extra
            }
            XDV_GLYPHS => 6 + be(*p + 4, 2)? * 10,
            XDV_TEXT_AND_GLYPHS => {
                let n = be(*p, 2)?;
                let q = 2 + n * 2 + 4;
                q + 2 + be(*p + q, 2)? * 10
            }
            PTEXDIR => 1,
            _ => return Some(false), // POST
        };
        *p += skip;
        Some(true)
    };
    while p < x.len() && step(&mut p) == Some(true) {}
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
        let mut map: Vec<(*const crate::pdffont::Used, crate::pdffont::UsedChars)> = Vec::new();
        let mut fresh = |uc: &mut Option<crate::pdffont::UsedChars>| {
            if let Some(old) = uc.take() {
                let p = Arc::as_ptr(&old);
                let new = match map.iter().find(|(q, _)| *q == p) {
                    Some((_, n)) => n.clone(),
                    None => {
                        let n = crate::pdffont::Used::of(&old.to_vec());
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

#[cfg(test)]
mod tests {
    //! Corrupt input stops the run with an `Err` (C's `ERROR`) or C's
    //! warning, never a panic.

    use super::*;
    use alloc::string::ToString;

    /// The files by name, found as themselves.
    struct MemFiles(Vec<(&'static [u8], Vec<u8>)>);

    impl Files for MemFiles {
        fn find(&self, name: &[u8], _: crate::io::Format, _: &[u8]) -> Option<Vec<u8>> {
            self.0
                .iter()
                .any(|(n, _)| *n == name)
                .then(|| name.to_vec())
        }
        fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
            self.0
                .iter()
                .find(|(n, _)| *n == path)
                .map(|(_, d)| Arc::from(d.as_slice()))
        }
    }

    /// An XDV preamble (`pre`, XDV id 7, TeX's units, no comment).
    fn preamble() -> Vec<u8> {
        let mut p = vec![crate::dvi::PRE, crate::dvi::XDV_ID];
        p.extend(25_400_000u32.to_be_bytes());
        p.extend(473_628_672u32.to_be_bytes());
        p.extend(1000u32.to_be_bytes());
        p.push(0);
        p
    }

    /// A page (`bop` .. `eop`) with one `\special`.
    fn page(special: &[u8]) -> Vec<u8> {
        let mut p = vec![crate::dvi::BOP];
        p.extend([0u8; 40]);
        p.extend((-1i32).to_be_bytes());
        p.push(crate::dvi::XXX4);
        p.extend(u32::try_from(special.len()).unwrap().to_be_bytes());
        p.extend_from_slice(special);
        p.push(crate::dvi::EOP);
        p
    }

    /// The fatal error of a one-page run, and the bytes written before
    /// it.
    fn run(
        special: &[u8],
        files: Vec<(&'static [u8], Vec<u8>)>,
    ) -> Result<Vec<u8>, (Fatal, Vec<u8>)> {
        let options = Options {
            source_date_epoch: Some(0),
            ..Options::default()
        };
        let deflate: Deflate = Box::new(|_, d: &[u8]| d.to_vec());
        let mut s = Session::new(options, Arc::new(MemFiles(files)), deflate, &preamble())
            .map_err(|e| (e, Vec::new()))?;
        let mut out = Vec::new();
        let r = s.page(&page(special)).map(|p| out.extend(p.pdf));
        match r.and_then(|()| s.finish()) {
            Ok(rest) => {
                out.extend(rest);
                Ok(out)
            }
            Err(e) => {
                out.extend(s.written());
                Err((e, out))
            }
        }
    }

    const PNG: &[u8] = include_bytes!("../../../../tests/e2e/png-rgb8.png");
    const JPEG: &[u8] = include_bytes!("../../../../tests/e2e/xpic-exif300.jpg");
    const PDF: &[u8] = include_bytes!("../../../../tests/e2e/xpic-three.pdf");

    const CMR10_TFM: &[u8] = include_bytes!("../../../fonts/tfm/tests/fixtures/cmr10.tfm");
    const CMR10_PFB: &[u8] = include_bytes!("../../pdf/tests/fixtures/cmr10.pfb");

    /// One page showing "A" in cmr10, and the PDF and the page's glyph
    /// runs with `glyph_runs` as given; the end's streams compressed in a
    /// batch (backwards, as a parallel host may) if `batch`.
    fn cmr10_page(glyph_runs: bool, batch: bool) -> (Vec<u8>, Vec<GlyphRun>, usize) {
        // (the map line first: the font embedded from its Type 1 file)
        let mut p = page(b"pdf:mapline cmr10 CMR10 <cmr10.pfb");
        p.pop();
        p.extend([crate::dvi::FNT_DEF1, 0]);
        p.extend_from_slice(&CMR10_TFM[24..28]);
        p.extend(655_360u32.to_be_bytes());
        p.extend(655_360u32.to_be_bytes());
        p.extend([0, 5]);
        p.extend_from_slice(b"cmr10");
        p.extend([crate::dvi::FNT_NUM_0, b'A', crate::dvi::EOP]);
        /// cmr10's metrics and Type 1 program, whatever the suffix asked.
        struct Cmr10;
        impl Files for Cmr10 {
            fn find(&self, name: &[u8], f: crate::io::Format, _: &[u8]) -> Option<Vec<u8>> {
                use crate::io::Format;
                if !name.starts_with(b"cmr10") {
                    return None;
                }
                match f {
                    Format::Tfm => Some(b"cmr10.tfm".to_vec()),
                    Format::Type1 => Some(b"cmr10.pfb".to_vec()),
                    _ => None,
                }
            }
            fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
                match path {
                    b"cmr10.tfm" => Some(Arc::from(CMR10_TFM)),
                    b"cmr10.pfb" => Some(Arc::from(CMR10_PFB)),
                    _ => None,
                }
            }
        }
        let files = Cmr10;
        let options = Options {
            source_date_epoch: Some(0),
            glyph_runs,
            ..Options::default()
        };
        let deflate: Deflate = Box::new(|l, d: &[u8]| phitex_codecs::deflate::compress(d, l));
        let mut s = Session::new(options, Arc::new(files), deflate, &preamble()).expect("starts");
        let batched = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        if batch {
            let n = batched.clone();
            s.set_batch_deflate(Box::new(move |jobs: &[(i32, &[u8])]| {
                n.fetch_add(jobs.len(), core::sync::atomic::Ordering::Relaxed);
                let mut out: Vec<Vec<u8>> = jobs
                    .iter()
                    .rev()
                    .map(|&(l, d)| phitex_codecs::deflate::compress(d, l))
                    .collect();
                out.reverse();
                out
            }));
        }
        let page = s.page(&p).unwrap_or_else(|e| panic!("page: {e}"));
        let mut pdf = page.pdf;
        pdf.extend(s.finish().unwrap_or_else(|e| panic!("finish: {e}")));
        (
            pdf,
            page.glyph_runs,
            batched.load(core::sync::atomic::Ordering::Relaxed),
        )
    }

    #[test]
    fn glyph_runs_only_when_asked() {
        let (pdf_on, runs_on, _) = cmr10_page(true, false);
        let (pdf_off, runs_off, _) = cmr10_page(false, false);
        assert_eq!(runs_on.len(), 1, "{runs_on:?}");
        assert!(
            matches!(&runs_on[0].source, GlyphSource::Type1 { glyph_name, .. } if glyph_name == b"A"),
            "{runs_on:?}"
        );
        assert!(runs_off.is_empty());
        assert!(pdf_on == pdf_off, "the PDF does not depend on the runs");
    }

    #[test]
    fn batch_deflate_same_bytes() {
        let (one, _, _) = cmr10_page(false, false);
        let (batch, _, n) = cmr10_page(false, true);
        assert!(one == batch, "the batch's PDF differs");
        // (the font program and the last object stream, at least)
        assert!(n >= 2, "{n} streams in the batch");
    }

    #[test]
    fn truncated_xdv() {
        let mut xdv = preamble();
        xdv.extend(page(b"pdf:literal 0 g"));
        for n in 0..xdv.len() {
            let options = Options::default();
            let deflate: Deflate = Box::new(|_, d: &[u8]| d.to_vec());
            let r = Session::convert(options, Arc::new(MemFiles(Vec::new())), deflate, &xdv[..n]);
            // (no page: "No pages fall in range!"; no preamble: dvi_init's)
            assert!(r.is_err(), "{n}");
        }
    }

    #[test]
    fn whole_images() {
        for (name, data) in [(&b"a.png"[..], PNG), (b"a.jpg", JPEG), (b"a.pdf", PDF)] {
            let mut s = b"pdf:image (".to_vec();
            s.extend_from_slice(name);
            s.push(b')');
            let pdf = run(&s, vec![(name, data.to_vec())]).expect("converts");
            assert!(pdf.starts_with(b"%PDF-1.5\n") && pdf.ends_with(b"%%EOF\n"));
        }
    }

    #[test]
    fn truncated_png() {
        let (e, out) = run(
            b"pdf:image (a.png)",
            vec![(b"a.png", PNG[..PNG.len() / 2].to_vec())],
        )
        .expect_err("fatal");
        assert_eq!(e.message, "libpng error: Read Error");
        assert_eq!(e.to_string(), "xdvipdfmx:fatal: libpng error: Read Error");
        // the header, written when the first page began
        assert!(out.starts_with(b"%PDF-1.5\n"));
    }

    #[test]
    fn truncated_jpeg() {
        let (e, _) =
            run(b"pdf:image (a.jpg)", vec![(b"a.jpg", JPEG[..40].to_vec())]).expect_err("fatal");
        assert_eq!(e.message, "Image inclusion failed for \"a.jpg\"");
    }

    #[test]
    fn bad_pdf_xref() {
        // the xref's first entry no longer a number: no object is found
        let at = PDF.windows(4).position(|w| w == b"xref").unwrap() + 5;
        let mut bad = PDF.to_vec();
        bad[at..at + 3].copy_from_slice(b"x y");
        let (e, _) = run(b"pdf:image (a.pdf)", vec![(b"a.pdf", bad)]).expect_err("fatal");
        assert_eq!(e.message, "Image inclusion failed for \"a.pdf\" (page=1).");
    }

    #[test]
    fn page_out_of_range() {
        let (e, _) =
            run(b"pdf:image page 9 (a.pdf)", vec![(b"a.pdf", PDF.to_vec())]).expect_err("fatal");
        assert_eq!(e.message, "Image inclusion failed for \"a.pdf\" (page=9).");
    }

    #[test]
    fn malformed_pdf_specials() {
        // C's warnings: the special is skipped, the run goes on
        for s in [
            &b"pdf:put @nothing << /A 1 >>"[..],
            b"pdf:obj @a << /A [1 2",
            b"pdf:literal",
            b"pdf:bcolor [1 0",
            b"pdf:image",
            b"pdf:dest (x) [@thispage /XYZ",
            b"pdf:ann width 1x << >>",
        ] {
            run(s, Vec::new()).unwrap_or_else(|(e, _)| panic!("{s:?}: {e}"));
        }
        // C's ERRORs
        let (e, _) = run(b"pdf:bxobj @x width 1pt height 1pt", Vec::new()).expect_err("fatal");
        assert_eq!(e.message, "A pending form XObject at the end of page.");
        let (e, _) = run(b"pdf:bc /Pattern @nope", Vec::new()).expect_err("fatal");
        assert_eq!(e.message, "pdf_link_obj(): passed invalid object.");
        let (e, _) = run(b"pdf:obj @z [@prevpage]", Vec::new()).expect_err("fatal");
        assert_eq!(
            e.message,
            "Reference to previous page, but no pages have been completed yet."
        );
    }
}
