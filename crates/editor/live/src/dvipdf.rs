//! A DVI file made a PDF in process (`phitex-xdvipdfmx`), and the files
//! XeTeX's PDF reads, through a host's finder ([`DpxFinder`]): the CLI's
//! kpathsea (as `dvipdfmx` finds them), a wasm host's own files (the
//! frontends' Shelf). For the viewer: a DVI build's output is the DVI
//! file; XeTeX's is its PDF, which the engine writes from its pages as
//! xelatex has xdvipdfmx make it (`phitex_tex::XdvPdf`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use phitex_xdvipdfmx::api::{Options, Session};
pub use phitex_xdvipdfmx::io::{Files, Format};

/// A lookup: `find(name, format, progname)`, kpathsea's
/// `kpse_find_file(name, format, 0)` as program `progname` (dvipdfmx
/// searches as `dvipdfmx`, and as others for some formats): the path
/// found.
pub type Find = Box<dyn Fn(&[u8], Format, &[u8]) -> Option<Vec<u8>> + Send + Sync>;

/// A read: the bytes of a path [`Find`] gave, or of an absolute path the
/// DVI names.
pub type Read = Box<dyn Fn(&[u8]) -> Option<Arc<[u8]>> + Send + Sync>;

/// A fresh view of a host's files for each link of XeTeX's pages
/// ([`crate::host::Host::xdv_files`], [`phitex_tex::XdvPdf::files`]).
pub type FilesFor = Arc<dyn Fn() -> Arc<dyn Files> + Send + Sync>;

/// A host's files for dvipdfmx ([`crate::host::Host::dpx_finder`]).
pub struct DpxFinder {
    pub find: Find,
    pub read: Read,
}

impl DpxFinder {
    pub fn new(find: Find, read: Read) -> DpxFinder {
        DpxFinder { find, read }
    }

    /// dvipdfmx's files over the finder: each lookup and read once a
    /// conversion (a link of XeTeX's pages).
    pub fn files(self: &Arc<Self>) -> Arc<dyn Files> {
        Arc::new(Cached {
            finder: self.clone(),
            found: Mutex::default(),
            read: Mutex::default(),
        })
    }
}

/// Lookups and reads, each once.
type Found = HashMap<(Vec<u8>, Format, Vec<u8>), Option<Vec<u8>>>;

struct Cached {
    finder: Arc<DpxFinder>,
    found: Mutex<Found>,
    read: Mutex<HashMap<Vec<u8>, Option<Arc<[u8]>>>>,
}

impl Files for Cached {
    fn find(&self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let f = &self.finder;
        self.found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((name.to_vec(), format, progname.to_vec()))
            .or_insert_with(|| (f.find)(name, format, progname))
            .clone()
    }

    fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
        let f = &self.finder;
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(path.to_vec())
            .or_insert_with(|| (f.read)(path))
            .clone()
    }
}

/// The PDF dvipdfmx makes of `dvi` (`dvipdfmx -q`, named `pdf_name`,
/// dated `date`), its files from `files`, or why not.
///
/// # Errors
/// dvipdfmx's.
pub fn dvi_to_pdf(
    dvi: &[u8],
    pdf_name: &str,
    date: i64,
    files: Arc<dyn Files>,
) -> Result<Vec<u8>, String> {
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
        files,
        Box::new(|level, data| phitex_codecs::deflate::compress(data, level)),
        dvi,
    )
    .map_err(|e| format!("dvipdfmx: {e}"))
}
