//! The machine phitex-live's session runs on here ([`phitex_live::host`]):
//! the files Shelf's and the host's ([`Shelf`], over [`crate::files`]), the
//! outputs kept in memory for the host to read ([`Memory`]); the store is
//! [`crate::store`]. Nothing is read from or written to a disk: there is
//! none.
//!
//! XeTeX's fonts ([`Shelf`]'s `xetex`) as the native Shelf host
//! gives them (crates/cli/src/shelf.rs): the release's font index, the
//! faces and `[file]` names from Shelf's packs, the project's own first.
//!
//! XeTeX's PDF (xelatex's xdvipdfmx, in process) reads its files through
//! [`Shelf`]'s `dpx_finder` the same way: the project's, else Shelf's.
//!
//! What a native host gives and this one does not (the default): a
//! failure's report file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use phitex_live::host::{Host, Sink};
use phitex_live::session::Job;

use crate::files::Files;

/// The files the build reads: Shelf's packs and the host's files
/// ([`Files`]), resolved for `engine` (Shelf's engine name); `\write18`'s
/// allowed commands posted to the host where it has a runner.
pub struct Shelf {
    pub files: Arc<Files>,
    pub engine: &'static str,
    pub shell: Option<Arc<dyn phitex_tex::ShellHook>>,
}

/// Shelf's font index in TeX Live's tree (`FONT_INDEX`,
/// crates/cli/src/shelf.rs: `phitex-otf`'s PXFI, a pack of its own).
const FONT_INDEX: &str = "fonts/misc/partex/fontindex.pxfi";

/// The release's font index ([`FONT_INDEX`]), its relative paths named
/// under TeX Live's tree; none: the release has none, or it is not one.
fn font_index(files: &Arc<Files>, engine: &'static str) -> Option<phitex_tex::FontIndex> {
    let (_, b) = files.shelf_find(&format!("{}{FONT_INDEX}", phitex_shelf::TL), None, engine)?;
    let mut ix = phitex_tex::FontIndex::from_bytes(&b)?;
    for e in &mut ix.entries {
        if !e.path.starts_with('/') {
            e.path = Arc::from(format!("{}{}", phitex_shelf::TL, e.path));
        }
    }
    Some(ix)
}

/// kpathsea's suffix for XeTeX's font `format`, added to a name without one.
fn font_suffix(format: phitex_tex::KpseFormat) -> &'static str {
    match format {
        phitex_tex::KpseFormat::OpenType => ".otf",
        phitex_tex::KpseFormat::TrueType => ".ttf",
        phitex_tex::KpseFormat::Type1 => ".pfb",
        phitex_tex::KpseFormat::MiscFonts => "",
    }
}

/// Ghostscript's font `name` (a file name, no directory: `NimbusRoman-Regular`)
/// where Shelf serves Ghostscript 10.08.0's own ([`phitex_shelf::GS_FONTS`]),
/// the bytes the EPS converter embeds as gs does; none before a release has them.
fn gs_font(files: &Arc<Files>, name: &str, engine: &'static str) -> Option<(String, Arc<Vec<u8>>)> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    files.shelf_find(&format!("{}/{name}", phitex_shelf::GS_FONTS), None, engine)
}

/// dvipdfmx's kpathsea format name for `format`, as Shelf's index has it.
fn dpx_format(format: phitex_live::dvipdf::Format) -> &'static str {
    use phitex_live::dvipdf::Format as F;
    match format {
        F::Fontmap => "map",
        F::Type1 => "type1",
        F::TrueType => "truetype",
        F::OpenType => "opentype",
        F::Enc => "enc",
        F::Tfm => "tfm",
        F::Vf => "vf",
        F::Sfd => "sfd",
        F::Ofm => "ofm",
        F::Ovf => "ovf",
        F::Cmap => "cmap",
        F::Pict => "pict",
        F::Tex => "tex",
        F::ProgramText => "other text files",
        F::ProgramBinary => "other binary files",
        F::Pk => "pk",
    }
}

/// The suffix kpathsea adds in dvipdfmx's `format` to a name without one.
fn dpx_suffix(format: phitex_live::dvipdf::Format) -> &'static str {
    use phitex_live::dvipdf::Format as F;
    match format {
        F::Fontmap => ".map",
        F::Type1 => ".pfb",
        F::TrueType => ".ttf",
        F::OpenType => ".otf",
        F::Enc => ".enc",
        F::Tfm => ".tfm",
        F::Vf => ".vf",
        F::Sfd => ".sfd",
        F::Ofm => ".ofm",
        F::Ovf => ".ovf",
        // (a PK font's request names its resolution: cmr10.600pk)
        F::Cmap | F::Pict | F::Tex | F::ProgramText | F::ProgramBinary | F::Pk => "",
    }
}

impl Host for Shelf {
    fn locate(&self, name: &str) -> Option<PathBuf> {
        self.files.locate(name, self.engine).map(PathBuf::from)
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        self.files.read(&path.to_string_lossy()).map(|b| b.to_vec())
    }

    /// Shelf's files keyed by their pack and path, none read: a warm start
    /// checks the format against them without fetching them
    /// ([`Files::key`]).
    fn content_key(&self) -> Option<phitex_tex::ContentKey> {
        let (f, e) = (self.files.clone(), self.engine);
        Some(Arc::new(move |name: &str| f.key(name, e)))
    }

    /// BibTeX's and makeindex's files, as kpathsea finds them in their
    /// formats (the format's suffix added where the name has none): the
    /// project's, else Shelf's.
    fn tool_finder(&self) -> Option<phitex_tex::ToolFinder> {
        let (f, e) = (self.files.clone(), self.engine);
        Some(Arc::new(
            move |search: phitex_tex::ToolSearch, name: &str| {
                let ext = match search {
                    phitex_tex::ToolSearch::BibStyle => ".bst",
                    phitex_tex::ToolSearch::BibData => ".bib",
                    phitex_tex::ToolSearch::IndexStyle => ".ist",
                    // (Ghostscript's fonts by their file names, where Shelf serves them)
                    phitex_tex::ToolSearch::GsFont => {
                        return gs_font(&f, name, e).map(|(p, b)| (p, b.to_vec()));
                    }
                    // (METAFONT's inputs and web2c's files for a font the engine
                    // makes, mktex: in their formats whatever the suffix, mf.ini,
                    // cp227.tcx; METAFONT's with the project's own first)
                    phitex_tex::ToolSearch::Mf => {
                        if let Some(p) = f.given(name) {
                            return f.read(&p).map(|b| (p, b.to_vec()));
                        }
                        return f
                            .shelf_find(name, Some("mf"), e)
                            .map(|(p, b)| (p, b.to_vec()));
                    }
                    phitex_tex::ToolSearch::Web2c => {
                        return f
                            .shelf_find(name, Some("web2c"), e)
                            .map(|(p, b)| (p, b.to_vec()));
                    }
                };
                let name = if name.ends_with(ext) {
                    name.to_owned()
                } else {
                    format!("{name}{ext}")
                };
                let p = f.locate(&name, e)?;
                let b = f.read(&p)?;
                Some((p, b.to_vec()))
            },
        ))
    }

    fn shell_hook(&self, _job: &Job) -> Option<Arc<dyn phitex_tex::ShellHook>> {
        self.shell.clone()
    }

    /// dvipdfmx's files (XeTeX's PDF): the project's (pictures), else
    /// Shelf's by name in its format (its suffix added where the name has
    /// none).
    fn dpx_finder(&self) -> Option<Arc<phitex_live::dvipdf::DpxFinder>> {
        let (f1, f2, f3, e) = (
            self.files.clone(),
            self.files.clone(),
            self.files.clone(),
            self.engine,
        );
        Some(Arc::new(
            phitex_live::dvipdf::DpxFinder::new(
                Box::new(move |name, format, _prog| {
                    let n = String::from_utf8_lossy(name).into_owned();
                    if let Some(p) = f1.given(&n) {
                        return Some(p.into_bytes());
                    }
                    let suffix = dpx_suffix(format);
                    let n = if suffix.is_empty() || n.contains('.') {
                        n
                    } else {
                        format!("{n}{suffix}")
                    };
                    f1.shelf_find(&n, Some(dpx_format(format)), e)
                        .map(|(p, _)| p.into_bytes())
                }),
                Box::new(move |path| {
                    let p = String::from_utf8_lossy(path).into_owned();
                    match f2.read(&p) {
                        Some(b) => Some(Arc::from(b.as_slice())),
                        None if p.starts_with(phitex_shelf::TL) => f2
                            .shelf_find(&p, None, e)
                            .map(|(_, b)| Arc::from(b.as_slice())),
                        None => None,
                    }
                }),
            )
            // (the `D` line's distiller: Ghostscript's fonts, where Shelf serves them)
            .with_gs_font(Box::new(move |name| {
                gs_font(&f3, std::str::from_utf8(name).ok()?, e)
                    .map(|(_, b)| Arc::from(b.as_slice()))
            })),
        ))
    }

    /// XeTeX's fonts: the release's font index ([`font_index`]), the faces
    /// and `[file]` names from Shelf's packs (the project's own first).
    fn xetex(&self) -> Option<phitex_tex::XeTeX> {
        let ix = font_index(&self.files, self.engine)?;
        let (f1, f2, e) = (self.files.clone(), self.files.clone(), self.engine);
        Some(phitex_live::xefonts::xetex_of(
            Box::new(move |path| match f1.read(path) {
                Some(b) => Some(Arc::from(b.as_slice())),
                None if path.starts_with(phitex_shelf::TL) => f1
                    .shelf_find(path, None, e)
                    .map(|(_, b)| Arc::from(b.as_slice())),
                None => None,
            }),
            Box::new(move |name, format| {
                if let Some(p) = f2.given(name) {
                    return Some(p);
                }
                let suffix = font_suffix(format);
                let kind = match format {
                    phitex_tex::KpseFormat::OpenType => "opentype",
                    phitex_tex::KpseFormat::TrueType => "truetype",
                    phitex_tex::KpseFormat::Type1 => "type1",
                    phitex_tex::KpseFormat::MiscFonts => "misc",
                };
                if !suffix.is_empty()
                    && !name.contains('.')
                    && let Some((p, _)) = f2.shelf_find(&format!("{name}{suffix}"), Some(kind), e)
                {
                    return Some(p);
                }
                f2.shelf_find(name, Some(kind), e).map(|(p, _)| p)
            }),
            Some(ix),
        ))
    }
}

/// The outputs, by name, in memory: the main output shared with the
/// engine's bytes (not copied), the rest as written. Read by the host
/// (`ph_file`) from its thread 0.
#[derive(Default)]
pub struct Memory(Mutex<HashMap<String, Arc<Vec<u8>>>>);

impl Memory {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<Vec<u8>>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// File `name` as last written (none: not written).
    pub fn file(&self, name: &str) -> Option<Arc<Vec<u8>>> {
        self.lock()
            .get(name.strip_prefix("./").unwrap_or(name))
            .cloned()
    }
}

impl Sink for Memory {
    fn begin(&self) -> Result<(), String> {
        Ok(())
    }

    fn put(&self, name: &str, bytes: &[u8], _hash: u64) -> Result<PathBuf, String> {
        self.lock()
            .insert(name.to_owned(), Arc::new(bytes.to_vec()));
        Ok(PathBuf::from(name))
    }

    fn put_main(&self, name: &str, bytes: &Arc<Vec<u8>>, _hash: u64) -> Result<PathBuf, String> {
        self.lock().insert(name.to_owned(), bytes.clone());
        Ok(PathBuf::from(name))
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.put(name, bytes, 0).map(|_| ())
    }

    fn remove(&self, name: &str) {
        self.lock().remove(name);
    }

    fn get(&self, name: &str) -> Option<Vec<u8>> {
        self.file(name).map(|b| b.to_vec())
    }

    fn path(&self, name: &str) -> PathBuf {
        PathBuf::from(name)
    }
}
