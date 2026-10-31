//! A DVI file made a PDF (`phitex_live::dvipdf`, dvipdfmx in process),
//! and the files XeTeX's PDF reads, through kpathsea as dvipdfmx finds
//! them: the CLI's finder; a wasm host gives its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use phitex_kpse::{Format as KpseFormat, Kpse};
use phitex_live::dvipdf::{Files, Format};

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
        Format::Pk => KpseFormat::Pk,
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

impl DpxFiles {
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let key = (name.to_vec(), format, progname.to_vec());
        if let Some(r) = self.found.get(&key) {
            return r.clone();
        }
        self.kpse.reset_program_name(progname);
        let r = if format == Format::Pk {
            // (`kpse_find_glyph`, mktexpk on a miss)
            let request = String::from_utf8_lossy(name);
            self.kpse.find_pk_request(&request).or_else(|| {
                let (font, dpi) = phitex_kpse::pk_name(&request)?;
                let (bdpi, mode) = phitex_kpse::pk_settings(&request)?;
                let p = crate::mkfont::mktexpk(font, dpi, bdpi, mode)?;
                Some(
                    <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::as_bytes(p.as_os_str())
                        .to_vec(),
                )
            })
        } else {
            self.kpse.find_file(name, kpse_format(format), false)
        };
        // (kpathsea's mktextfm on a TFM it finds none of)
        let r = match (r, format) {
            (None, Format::Tfm) => {
                let n = String::from_utf8_lossy(name);
                crate::mkfont::mktextfm(n.strip_suffix(".tfm").unwrap_or(&n)).map(|p| {
                    <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::as_bytes(p.as_os_str())
                        .to_vec()
                })
            }
            (r, _) => r,
        };
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

/// One process's [`DpxFiles`]: its lookups and the files read kept for
/// every conversion and link.
fn shared() -> Arc<Mutex<DpxFiles>> {
    static DPX: std::sync::OnceLock<Arc<Mutex<DpxFiles>>> = std::sync::OnceLock::new();
    DPX.get_or_init(|| Arc::new(Mutex::new(DpxFiles::new())))
        .clone()
}

/// [`DpxFiles`] shared.
struct Shared(Arc<Mutex<DpxFiles>>);

impl Files for Shared {
    fn find(&self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .find(name, format, progname)
    }

    fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .read(path)
    }
}

/// The PDF dvipdfmx makes of `dvi` (named `pdf_name`), or why not.
pub fn dvi_to_pdf(dvi: &[u8], pdf_name: &str, date: i64) -> Result<Vec<u8>, String> {
    phitex_live::dvipdf::dvi_to_pdf(dvi, pdf_name, date, Arc::new(Shared(shared())))
}

/// dvipdfmx's files for an XDV file: names relative to the document's
/// directory found there first (xelatex runs xdvipdfmx in the
/// directory its pictures are named from), the rest as [`DpxFiles`],
/// whose lookups and reads one process keeps.
struct XdvFiles {
    dir: std::path::PathBuf,
    dpx: Arc<Mutex<DpxFiles>>,
}

impl Files for XdvFiles {
    fn find(&self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let rel = std::path::Path::new(
            <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(name),
        );
        if rel.is_relative() && matches!(format, Format::Pict | Format::Tex | Format::ProgramBinary)
        {
            let p = self.dir.join(rel);
            if p.is_file() {
                return Some(
                    <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::as_bytes(p.as_os_str())
                        .to_vec(),
                );
            }
        }
        let mut d = self.dpx.lock().unwrap_or_else(PoisonError::into_inner);
        d.find(name, format, progname)
    }

    fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
        // (the document's own files read again each time: they change
        // between a watch's builds)
        let p = std::path::Path::new(
            <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path),
        );
        if p.is_relative() || p.starts_with(&self.dir) {
            return std::fs::read(self.dir.join(p)).ok().map(Arc::from);
        }
        let mut d = self.dpx.lock().unwrap_or_else(PoisonError::into_inner);
        d.read(path)
    }
}

/// The files XeTeX's PDF reads, its pictures named from `dir`: a fresh
/// view for each link ([`XdvFiles`]).
pub fn xdv_files(dir: &std::path::Path) -> phitex_live::dvipdf::FilesFor {
    let dir = dir.to_path_buf();
    Arc::new(move || {
        Arc::new(XdvFiles {
            dir: dir.clone(),
            dpx: shared(),
        }) as Arc<dyn Files>
    })
}
