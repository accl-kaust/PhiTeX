//! What a host gives a session: everything that touches its machine. The
//! session itself reads no file, writes none, runs no process and asks no
//! clock but `Instant`'s; the CLI gives it kpathsea, the cache directory
//! and the output directory (`crates/cli/src/session.rs`), the frontends'
//! wasm core Shelf, the host's store and the host's memory.
//!
//! - [`Host`]: the files the build reads (by the names TeX gives them, as
//!   found, and their bytes), the format's and the tools' finders, the
//!   machine's services a build may call (`\write18`'s commands, XeTeX's
//!   fonts, xdvipdfmx, a failure's report).
//! - [`Store`]: the bytes the session keeps between sessions (the cache
//!   directory's, for the CLI): the format's evaluated state, the package
//!   loads' records beside it, the document's read-back keys, written
//!   names and nodes' records.
//! - [`Sink`]: where what the session writes goes (the output directory's
//!   files, for the CLI), and what an earlier run left there.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use phitex_tex::{Finder, ToolFinder};

use crate::session::Job;

/// The machine a session runs on.
pub trait Host: Send + Sync {
    /// Where the job's file `name` (as TeX names it) is found (kpathsea's
    /// lookup, in the format its extension names); none: nowhere.
    fn locate(&self, name: &str) -> Option<PathBuf>;

    /// The bytes of the file at `path` (as [`Host::locate`] found it, or
    /// as a diagnostic names it); none: not readable.
    fn read(&self, path: &Path) -> Option<Vec<u8>>;

    /// Whether two names are one file (none of the host's own: names are
    /// compared as they are).
    fn same_file(&self, _a: &str, _b: &str) -> bool {
        false
    }

    /// `path` as the host names it whole (the SyncTeX file's directory:
    /// the CLI's canonical path); as it is by default.
    fn canonical(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    /// Whether the format's ini file `name` is found where formats are
    /// built.
    fn has_format_file(&self, _name: &str) -> bool {
        true
    }

    /// The format's files, found as a format's build finds them (none:
    /// the job's finder).
    fn format_finder(&self) -> Option<Finder> {
        None
    }

    /// The format's files located as its finder finds them, each with a
    /// key of its content that needs no read (the CLI's stat): a warm
    /// start checks the format against them without reading them (none:
    /// they are read and hashed).
    fn content_key(&self) -> Option<phitex_tex::ContentKey> {
        None
    }

    /// The tools' support files (bibliography styles and databases, index
    /// styles).
    fn tool_finder(&self) -> Option<ToolFinder> {
        None
    }

    /// BibTeX's options (its capacities as the host's TeX sets them).
    fn bibtex(&self) -> phitex_bibtex::Options {
        phitex_bibtex::Options::default()
    }

    /// XeTeX's native fonts, for a job whose engine is XeTeX.
    fn xetex(&self) -> Option<phitex_tex::XeTeX> {
        None
    }

    /// What runs `\write18`'s commands that are neither the engine's own
    /// nor refused, for `job`.
    fn shell_hook(&self, _job: &Job) -> Option<Arc<dyn phitex_tex::ShellHook>> {
        None
    }

    /// dvipdfmx's files (fonts, maps, encodings, the document's
    /// pictures): what [`Host::dvi_to_pdf`] and [`Host::xdv_files`] read
    /// by default (none: no PDF of a DVI, none of XeTeX's pages).
    fn dpx_finder(&self) -> Option<Arc<crate::dvipdf::DpxFinder>> {
        None
    }

    /// The files XeTeX's PDF reads (xdvipdfmx's: fonts, maps, the
    /// document's pictures, named from `dir`), a fresh view for each
    /// link of the pages ([`phitex_tex::XdvPdf::files`]); by default
    /// [`Host::dpx_finder`]'s. None: no PDF, the build's XDV only.
    fn xdv_files(&self, _dir: &Path) -> Option<crate::dvipdf::FilesFor> {
        let f = self.dpx_finder()?;
        Some(Arc::new(move || f.files()))
    }

    /// A DVI file made a PDF named `pdf_name` (dvipdfmx), for a viewer
    /// (by default through [`Host::dpx_finder`]).
    ///
    /// # Errors
    /// Why it could not be made.
    fn dvi_to_pdf(&self, dvi: &[u8], pdf_name: &str, date: i64) -> Result<Vec<u8>, String> {
        match self.dpx_finder() {
            Some(f) => crate::dvipdf::dvi_to_pdf(dvi, pdf_name, date, f.files()),
            None => Err("no dvipdfmx here: the DVI file is not made a PDF".to_owned()),
        }
    }

    /// An engine failure kept for a bug report: the document's name, why
    /// the build ran, its threads, the engine's account and the errors
    /// shown. Where it was kept, as the reader is told (none: not kept).
    fn failure_report(
        &self,
        _file: &str,
        _why: &str,
        _threads: usize,
        _internal: &str,
        _errors: &[String],
    ) -> Option<String> {
        None
    }

    /// The process's resident set in kB (0: not known), for the build's
    /// notes.
    fn resident_kb(&self) -> u64 {
        0
    }

    /// A cold build's engine began, its progress `progress` (a host may
    /// watch it from outside, the CLI's `PHITEX_SNAPSHOT_SECS`).
    fn progress(&self, _progress: &Arc<phitex_tex::Progress>) {}
}

/// What the session keeps between sessions, by key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key<'a> {
    /// The document's own: `wkeys`, `wnames`, `noderec`, `toolmade`.
    Doc(&'a str),
    /// One beside format `name`: `(name, "pkgrec")`.
    Format(&'a str, &'a str),
}

/// The bytes kept between sessions (the CLI's cache directory).
pub trait Store: Send + Sync {
    fn get(&self, key: Key<'_>) -> Option<Vec<u8>>;
    fn put(&self, key: Key<'_>, bytes: &[u8]);
    /// Format `name`'s evaluated state and the key it was stored with.
    fn format(&self, name: &str) -> Option<([u8; 32], Vec<u8>)>;
    fn put_format(&self, name: &str, key: &[u8; 32], bytes: &[u8]);
}

/// The version of the encodings this crate gives the document's records
/// (`wkeys`, `wnames`: [`crate::session`]'s); the engine's own records
/// (`pkgrec`, `noderec`) are its, under [`phitex_tex::engine_id`]. Bumped
/// with any change to those encodings (`records_encodings_are_locked`
/// fails until it is).
pub const RECORDS_VERSION: u64 = 1;

/// The stamp every record is stored with: the engine's identity and
/// [`RECORDS_VERSION`]. A record under another stamp (another engine,
/// newer or older, or another encoding) is not read.
#[must_use]
pub fn records_stamp() -> u64 {
    phitex_tex::engine_id() ^ RECORDS_VERSION.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// The host's [`Store`] as the session reads and writes it: each record
/// ([`Key`]) stored behind its stamp ([`records_stamp`], 8 bytes, little
/// endian), and one under another stamp, or none, read as not there (the
/// session makes it again, and stores it over the other). The only way
/// the session reaches the store, so that no record goes unstamped; a
/// format is the store's own, under the key the engine gave it.
#[derive(Clone)]
pub struct Records {
    store: Arc<dyn Store>,
    stamp: u64,
}

impl Records {
    #[must_use]
    pub fn new(store: Arc<dyn Store>) -> Records {
        Records::stamped(store, records_stamp())
    }

    /// Records under stamp `stamp` (another engine's, in tests).
    #[must_use]
    pub fn stamped(store: Arc<dyn Store>, stamp: u64) -> Records {
        Records { store, stamp }
    }

    /// The same store under stamp `stamp`.
    #[must_use]
    pub fn restamped(&self, stamp: u64) -> Records {
        Records::stamped(self.store.clone(), stamp)
    }

    /// Record `key`, when it was stored under this stamp.
    #[must_use]
    pub fn get(&self, key: Key<'_>) -> Option<Vec<u8>> {
        let b = self.store.get(key)?;
        let (s, rest) = b.split_first_chunk::<8>()?;
        (u64::from_le_bytes(*s) == self.stamp).then(|| rest.to_vec())
    }

    /// Record `key` stored as `bytes`, under this stamp.
    pub fn put(&self, key: Key<'_>, bytes: &[u8]) {
        let mut b = Vec::with_capacity(8 + bytes.len());
        b.extend_from_slice(&self.stamp.to_le_bytes());
        b.extend_from_slice(bytes);
        self.store.put(key, &b);
    }

    #[must_use]
    pub fn format(&self, name: &str) -> Option<([u8; 32], Vec<u8>)> {
        self.store.format(name)
    }

    pub fn put_format(&self, name: &str, key: &[u8; 32], bytes: &[u8]) {
        self.store.put_format(name, key, bytes);
    }
}

/// Where the session's outputs go (the CLI's output directory), by name
/// in it. Each answers where the file is (a path, as the host names it).
pub trait Sink: Send + Sync {
    /// The output directory made, before a write.
    ///
    /// # Errors
    /// Why it could not be made.
    fn begin(&self) -> Result<(), String>;

    /// File `name` holds `bytes` (whose [`crate::tex::fingerprint`] is
    /// `hash`) from now on: a host may leave a file it wrote with them
    /// as it is.
    ///
    /// # Errors
    /// Why it could not be written.
    fn put(&self, name: &str, bytes: &[u8], hash: u64) -> Result<PathBuf, String>;

    /// The main output (the PDF or DVI), shared, its fingerprint `hash`:
    /// a host may write only what changed since the last it wrote.
    ///
    /// # Errors
    /// Why it could not be written.
    fn put_main(&self, name: &str, bytes: &Arc<Vec<u8>>, hash: u64) -> Result<PathBuf, String>;

    /// Whether the host keeps the PDF file as update sections
    /// ([`Sink::put_update`]): the build then gives the link's layout,
    /// not the exact bytes.
    fn updates(&self) -> bool {
        false
    }

    /// The PDF file `name` brought to layout `layout` (an update section
    /// appended where it holds an earlier one).
    ///
    /// # Errors
    /// Why it could not be written.
    fn put_update(&self, name: &str, _layout: &phitex_pdf::Layout) -> Result<PathBuf, String> {
        Err(format!("{name}: no update sections here"))
    }

    /// File `name` written whole, as it is (the log a watch writes when
    /// idle).
    ///
    /// # Errors
    /// Why it could not be written.
    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String>;

    /// The directory file `name` is in made (a `\write18` command's file
    /// in a subdirectory).
    ///
    /// # Errors
    /// Why it could not be made.
    fn dir_of(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }

    /// File `name` removed (a `\write18` command removed it).
    fn remove(&self, name: &str);

    /// File `name` as an earlier run left it (none: not there).
    fn get(&self, name: &str) -> Option<Vec<u8>>;

    /// Where file `name` is, as the recorder names it.
    fn path(&self, name: &str) -> PathBuf;

    /// The directory the job runs in, as the recorder's `PWD` names it.
    fn here(&self) -> PathBuf {
        PathBuf::new()
    }
}
