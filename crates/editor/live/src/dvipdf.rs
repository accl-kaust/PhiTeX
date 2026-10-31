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
    /// Ghostscript's fonts by file name, for the `D` line's built-in
    /// distiller (`Files::gs_font`).
    pub gs_font: Option<Read>,
}

impl DpxFinder {
    pub fn new(find: Find, read: Read) -> DpxFinder {
        DpxFinder {
            find,
            read,
            gs_font: None,
        }
    }

    /// With Ghostscript's fonts from `gs_font` ([`Files::gs_font`]).
    #[must_use]
    pub fn with_gs_font(mut self, gs_font: Read) -> DpxFinder {
        self.gs_font = Some(gs_font);
        self
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

    fn gs_font(&self, name: &[u8]) -> Option<Arc<[u8]>> {
        let g = self.finder.gs_font.as_ref()?;
        let mut key = b"gs-font:".to_vec();
        key.extend_from_slice(name);
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_insert_with(|| g(name))
            .clone()
    }
}

/// `files` with the job's own files before them, as kpathsea searches
/// for xdvipdfmx as for TeX: the job's directory (`.`, first in every
/// path of `texmf.cnf`) and then the tree. A relative name the job's
/// directory `dir` has is that file, read by the host (a picture of the
/// project, `./logo.pdf`; a project's own `.map` or `.enc` over TeX
/// Live's); any other is the finder's (the CLI's kpathsea, the frontends'
/// Shelf).
pub fn with_job_files(
    files: FilesFor,
    host: Arc<dyn crate::host::Host>,
    dir: std::path::PathBuf,
) -> FilesFor {
    Arc::new(move || -> Arc<dyn Files> {
        Arc::new(JobFiles {
            files: files(),
            host: host.clone(),
            dir: dir.clone(),
            read: Mutex::default(),
        })
    })
}

/// [`with_job_files`]'s view: the job's files found, by path, with
/// their bytes.
struct JobFiles {
    files: Arc<dyn Files>,
    host: Arc<dyn crate::host::Host>,
    dir: std::path::PathBuf,
    read: Mutex<HashMap<Vec<u8>, Arc<[u8]>>>,
}

impl Files for JobFiles {
    fn find(&self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>> {
        let n = String::from_utf8_lossy(name);
        if !n.starts_with('/') {
            let p = self.dir.join(n.trim_start_matches("./"));
            if let Some(b) = self.host.read(&p) {
                let key = p.to_string_lossy().into_owned().into_bytes();
                self.read
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(key.clone(), b.into());
                return Some(key);
            }
        }
        self.files.find(name, format, progname)
    }

    fn read(&self, path: &[u8]) -> Option<Arc<[u8]>> {
        let kept = self
            .read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(path)
            .cloned();
        kept.or_else(|| self.files.read(path))
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

    /// The project's files, as the host reads them: a picture and its
    /// own font map.
    struct Project;

    impl crate::host::Host for Project {
        fn locate(&self, _name: &str) -> Option<PathBuf> {
            None
        }
        fn read(&self, path: &Path) -> Option<Vec<u8>> {
            match path.to_str()? {
                "/p/logo.pdf" => Some(b"%PDF-1.5 logo".to_vec()),
                "/p/pdftex.map" => Some(b"the project's map".to_vec()),
                _ => None,
            }
        }
    }

    /// The job's directory first, then the finder's tree, as kpathsea
    /// searches: a project file the tree lacks, one that shadows the
    /// tree's, and one only the tree has.
    #[test]
    fn the_jobs_files_come_before_the_finders() {
        let finder = Arc::new(DpxFinder::new(
            Box::new(|name, _, _| match name {
                b"pdftex.map" => Some(b"/tl/pdftex.map".to_vec()),
                b"cmr10.pfb" => Some(b"/tl/cmr10.pfb".to_vec()),
                _ => None,
            }),
            Box::new(|path| match path {
                b"/tl/pdftex.map" => Some(Arc::from(&b"TeX Live's map"[..])),
                b"/tl/cmr10.pfb" => Some(Arc::from(&b"pfb"[..])),
                _ => None,
            }),
        ));
        let f = with_job_files(
            Arc::new(move || finder.files()),
            Arc::new(Project),
            PathBuf::from("/p"),
        )();
        let found = |n: &[u8], k| f.find(n, k, b"dvipdfmx").map(|p| (p.clone(), f.read(&p)));
        let (p, b) = found(b"./logo.pdf", Format::Pict).expect("the project's picture");
        assert_eq!(
            (p.as_slice(), b.as_deref()),
            (&b"/p/logo.pdf"[..], Some(&b"%PDF-1.5 logo"[..]))
        );
        let (p, b) = found(b"pdftex.map", Format::Fontmap).expect("a map");
        assert_eq!(
            (p.as_slice(), b.as_deref()),
            (&b"/p/pdftex.map"[..], Some(&b"the project's map"[..])),
            "the project's own map over TeX Live's"
        );
        let (p, b) = found(b"cmr10.pfb", Format::Type1).expect("the tree's font");
        assert_eq!(
            (p.as_slice(), b.as_deref()),
            (&b"/tl/cmr10.pfb"[..], Some(&b"pfb"[..]))
        );
        assert_eq!(f.find(b"nothing.pdf", Format::Pict, b"dvipdfmx"), None);
    }
}
