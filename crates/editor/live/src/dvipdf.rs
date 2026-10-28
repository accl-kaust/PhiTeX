//! A DVI or XDV file made a PDF in process (`phitex-xdvipdfmx`), its
//! files through a host's finder ([`DpxFinder`]): the CLI's kpathsea (as
//! `dvipdfmx` finds them), a wasm host's own files (the frontends'
//! Shelf). For the viewer: a DVI build's output is the DVI file; XeTeX's
//! is the PDF xelatex has xdvipdfmx make of its XDV.

use std::collections::HashMap;
use std::sync::Arc;

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
    /// conversion.
    pub fn files(self: &Arc<Self>) -> Box<dyn Files> {
        Box::new(Cached {
            finder: self.clone(),
            found: HashMap::new(),
            read: HashMap::new(),
        })
    }
}

struct Cached {
    finder: Arc<DpxFinder>,
    found: HashMap<(Vec<u8>, Format, Vec<u8>), Option<Vec<u8>>>,
    read: HashMap<Vec<u8>, Option<Arc<[u8]>>>,
}

impl Files for Cached {
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let f = &self.finder;
        self.found
            .entry((name.to_vec(), format, progname.to_vec()))
            .or_insert_with(|| (f.find)(name, format, progname))
            .clone()
    }

    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>> {
        let f = &self.finder;
        self.read
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
pub fn dvi_to_pdf(dvi: &[u8], pdf_name: &str, date: i64, files: Box<dyn Files>) -> Result<Vec<u8>, String> {
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
    Session::convert(options, files, Box::new(|level, data| phitex_codecs::deflate::compress(data, level)), dvi)
        .map_err(|e| format!("dvipdfmx: {e}"))
}

/// The PDF xelatex has xdvipdfmx make of `xdv` (`xdvipdfmx -q -E -o
/// pdf_name`, dated `date`), its files (the document's pictures among
/// them) from `files`, or why not.
///
/// # Errors
/// xdvipdfmx's.
pub fn xdv_to_pdf(xdv: &[u8], pdf_name: &str, date: i64, files: Box<dyn Files>) -> Result<Vec<u8>, String> {
    let options = Options {
        pdf_filename: Some(pdf_name.as_bytes().to_vec()),
        source_date_epoch: Some(date),
        now: date,
        ..Options::default()
    };
    Session::convert(options, files, Box::new(|level, data| phitex_codecs::deflate::compress(data, level)), xdv)
        .map_err(|e| format!("xdvipdfmx: {e}"))
}
