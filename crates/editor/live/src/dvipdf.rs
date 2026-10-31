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

/// `files` with the job's own files behind them: a name dvipdfmx's
/// finder does not find (a picture of the project, `./logo.pdf`, where
/// the host's dvipdfmx files are TeX Live's alone: the frontends' Shelf)
/// is the file the host gives TeX itself by that name, read by the host,
/// as kpathsea finds the document's directory's files (`.` first in its
/// paths) for xdvipdfmx as for TeX.
pub fn with_job_files(files: FilesFor, host: Arc<dyn crate::host::Host>) -> FilesFor {
    Arc::new(move || -> Arc<dyn Files> {
        Arc::new(JobFiles {
            files: files(),
            host: host.clone(),
        })
    })
}

/// [`with_job_files`]'s view.
struct JobFiles {
    files: Arc<dyn Files>,
    host: Arc<dyn crate::host::Host>,
}

impl Files for JobFiles {
    fn find(&self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        self.files.find(name, format, progname).or_else(|| {
            let n = String::from_utf8_lossy(name);
            let p = self.host.locate(n.trim_start_matches("./"))?;
            Some(p.to_string_lossy().into_owned().into_bytes())
        })
    }

    fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
        self.files.read(path).or_else(|| {
            let p = std::path::PathBuf::from(String::from_utf8_lossy(path).into_owned());
            self.host.read(&p).map(Into::into)
        })
    }

    fn apply_filter(&self, template: &[u8], input: &[u8], version: i32) -> Option<Arc<[u8]>> {
        self.files.apply_filter(template, input, version)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The project's picture, as the host gives TeX its files.
    struct Project;

    impl crate::host::Host for Project {
        fn locate(&self, name: &str) -> Option<PathBuf> {
            (name == "logo.pdf").then(|| PathBuf::from("/p/logo.pdf"))
        }
        fn read(&self, path: &Path) -> Option<Vec<u8>> {
            (path == Path::new("/p/logo.pdf")).then(|| b"%PDF-1.5 logo".to_vec())
        }
    }

    /// A picture of the job's directory that dvipdfmx's finder (TeX
    /// Live's files alone) does not have is the job's file, read by the
    /// host; what the finder finds stays its.
    #[test]
    fn a_job_file_the_finder_lacks_is_the_hosts() {
        let finder = Arc::new(DpxFinder::new(
            Box::new(|name, _, _| (name == b"pdftex.map").then(|| b"/tl/pdftex.map".to_vec())),
            Box::new(|path| (path == b"/tl/pdftex.map").then(|| Arc::from(&b"map"[..]))),
        ));
        let f = with_job_files(Arc::new(move || finder.files()), Arc::new(Project))();
        let p = f
            .find(b"./logo.pdf", Format::Pict, b"dvipdfmx")
            .expect("found");
        assert_eq!(p, b"/p/logo.pdf");
        assert_eq!(f.read(&p).as_deref(), Some(&b"%PDF-1.5 logo"[..]));
        assert_eq!(
            f.find(b"pdftex.map", Format::Fontmap, b"dvipdfmx")
                .as_deref(),
            Some(&b"/tl/pdftex.map"[..])
        );
        assert_eq!(f.find(b"nothing.pdf", Format::Pict, b"dvipdfmx"), None);
    }
}
