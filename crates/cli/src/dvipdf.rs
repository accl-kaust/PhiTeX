//! A DVI file shown in the live viewer: made a PDF by dvipdfmx, in
//! process (`phitex-xdvipdfmx`), its files through kpathsea as dvipdfmx
//! finds them. Only for the viewer: the build's output is the DVI file.

use std::collections::HashMap;
use std::sync::Arc;

use phitex_kpse::{Format as KpseFormat, Kpse};
use phitex_xdvipdfmx::api::{Options, Session};
use phitex_xdvipdfmx::io::{Files, Format};

fn kpse_format(format: Format) -> KpseFormat {
    match format {
        Format::Fontmap => KpseFormat::FontMap,
        Format::Type1 => KpseFormat::Type1,
        Format::TrueType => KpseFormat::TrueType,
        Format::OpenType => KpseFormat::OpenType,
        Format::Cmap => KpseFormat::Cmap,
        Format::Sfd => KpseFormat::Sfd,
        Format::Enc => KpseFormat::Enc,
        Format::Tfm => KpseFormat::Tfm,
        Format::Ofm => KpseFormat::Ofm,
        Format::Vf => KpseFormat::Vf,
        Format::Ovf => KpseFormat::Ovf,
        Format::Pict => KpseFormat::Pict,
        Format::Tex => KpseFormat::Tex,
        Format::ProgramText => KpseFormat::ProgramText,
        Format::ProgramBinary => KpseFormat::ProgramBinary,
    }
}

/// dvipdfmx's files: kpathsea as `dvipdfmx` ("we pretend to be dvipdfmx
/// for kpse purposes"), each lookup and file once.
pub struct DpxFiles {
    kpse: Kpse,
    found: HashMap<(Vec<u8>, Format, Vec<u8>), Option<Vec<u8>>>,
    read: HashMap<Vec<u8>, Option<Arc<[u8]>>>,
}

impl DpxFiles {
    pub fn new() -> DpxFiles {
        DpxFiles {
            kpse: crate::tex::kpse("dvipdfmx", "pdftex"),
            found: HashMap::new(),
            read: HashMap::new(),
        }
    }
}

impl Files for DpxFiles {
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let key = (name.to_vec(), format, progname.to_vec());
        if let Some(r) = self.found.get(&key) {
            return r.clone();
        }
        self.kpse.reset_program_name(progname);
        let r = self.kpse.find_file(name, kpse_format(format), false);
        self.kpse.reset_program_name(b"dvipdfmx");
        self.found.insert(key, r.clone());
        r
    }

    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
        self.read
            .entry(path.to_vec())
            .or_insert_with(|| {
                let p = std::path::Path::new(
                    <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path),
                );
                std::fs::read(p).ok().map(Arc::from)
            })
            .clone()
    }
}

/// The PDF dvipdfmx makes of `dvi` (named `pdf_name`), or why not.
pub fn dvi_to_pdf(dvi: &[u8], pdf_name: &str, date: i64) -> Result<Vec<u8>, String> {
    let options = Options {
        pdf_filename: Some(pdf_name.as_bytes().to_vec()),
        dvi_filename: None,
        args: vec![b"-q".to_vec()],
        source_date_epoch: Some(date),
        now: date,
        utc_offset_min: 0,
        system_paper: None,
        glyph_runs: false,
    };
    Session::convert(
        options,
        Box::new(DpxFiles::new()),
        Box::new(|level, data| phitex_codecs::deflate::compress(data, level)),
        dvi,
    )
    .map_err(|e| format!("dvipdfmx: {e}"))
}

/// dvipdfmx's files for an XDV file: names relative to the document's
/// directory found there first (xelatex runs xdvipdfmx in the
/// directory its pictures are named from), the rest as [`DpxFiles`],
/// whose lookups and reads one process keeps.
struct XdvFiles {
    dir: std::path::PathBuf,
    dpx: Arc<std::sync::Mutex<DpxFiles>>,
}

impl Files for XdvFiles {
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let rel = std::path::Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(name));
        if rel.is_relative() && matches!(format, Format::Pict | Format::Tex | Format::ProgramBinary) {
            let p = self.dir.join(rel);
            if p.is_file() {
                return Some(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::as_bytes(p.as_os_str()).to_vec());
            }
        }
        let mut d = self.dpx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        d.find(name, format, progname)
    }

    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
        // (the document's own files read again each time: they change
        // between a watch's builds)
        let p = std::path::Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path));
        if p.is_relative() || p.starts_with(&self.dir) {
            return std::fs::read(self.dir.join(p)).ok().map(Arc::from);
        }
        let mut d = self.dpx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        d.read(path)
    }
}

/// The PDF xelatex has xdvipdfmx make of `xdv` (`xdvipdfmx -q -E -o
/// pdf_name`), its pictures named from `dir`, or why not.
pub fn xdv_to_pdf(xdv: &[u8], pdf_name: &str, date: i64, dir: &std::path::Path) -> Result<Vec<u8>, String> {
    static DPX: std::sync::OnceLock<Arc<std::sync::Mutex<DpxFiles>>> = std::sync::OnceLock::new();
    let dpx = DPX
        .get_or_init(|| Arc::new(std::sync::Mutex::new(DpxFiles::new())))
        .clone();
    let options = Options {
        pdf_filename: Some(pdf_name.as_bytes().to_vec()),
        source_date_epoch: Some(date),
        now: date,
        ..Options::default()
    };
    Session::convert(
        options,
        Box::new(XdvFiles {
            dir: dir.to_path_buf(),
            dpx,
        }),
        Box::new(|level, data| phitex_codecs::deflate::compress(data, level)),
        xdv,
    )
    .map_err(|e| format!("xdvipdfmx: {e}"))
}
