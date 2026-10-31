//! TeX Live as the builds see it: kpathsea, the finder a `Build` is given
//! (the engines by their program names are `phitex_live::tex`'s):
//! `\input` files by TEXINPUTS (from the environment, `.` first as
//! texmf.cnf has it), TFM files, map files, fonts and encodings, each a
//! lookup in the `ls-R` databases, no directory walked.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use phitex_kpse::{Format, Kpse};

use phitex_live::tex::samples;
pub use phitex_live::tex::{
    ANY_BYTES, Engine, Fnv, Output, chars, chars_owned, engine, fingerprint,
};

/// kpathsea as program `progname` of `engine`: `SELFAUTOLOC` is where the
/// executable lives; if no `texmf.cnf` is reachable from there (a
/// development build), the location of `tex` on `PATH`.
pub fn kpse(progname: &str, engine: &str) -> Kpse {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.canonicalize().ok());
    let own = exe.as_ref().and_then(|e| e.parent().map(Path::to_path_buf));
    if let Some(dir) = &own {
        let mut k = Kpse::new(dir, progname, engine);
        if k.var_value("TEXMF").is_some() {
            return k;
        }
    }
    let tex_dir = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("tex"))
            .filter_map(|t| t.canonicalize().ok())
            .find(|t| t.is_file() && Some(t) != exe.as_ref())
            .and_then(|t| t.parent().map(Path::to_path_buf))
    });
    let dir = tex_dir.or(own).unwrap_or_else(|| "/usr/bin".into());
    Kpse::new(&dir, progname, engine)
}

/// Whether this machine has a TeX Live kpathsea finds (a `texmf.cnf`
/// naming `TEXMF`), as [`kpse`] looks for it.
pub fn has_texlive() -> bool {
    kpse("pdftex", "pdftex").var_value("TEXMF").is_some()
}

/// [`fingerprint`] of the file at `p`, reading only what it needs.
pub fn fingerprint_file(p: &std::path::Path) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(p).ok()?;
    let len = usize::try_from(f.metadata().ok()?.len()).ok()?;
    if len <= 1 << 20 {
        let mut b = Vec::with_capacity(len);
        f.read_to_end(&mut b).ok()?;
        return Some(fingerprint(&b));
    }
    let mut h = Fnv::new().add(&(len as u64).to_le_bytes());
    let mut buf = Vec::new();
    for (at, n) in samples(len) {
        buf.resize(n, 0);
        f.seek(SeekFrom::Start(at as u64)).ok()?;
        f.read_exact(&mut buf).ok()?;
        h = h.add(&buf);
    }
    Some(h.0)
}

/// Where a run's TeX files come from (`files` in `phitex.toml`, the
/// editor's setting, `--shelf`): one source a run, never one after the
/// other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Files {
    /// TeX Live, by kpathsea.
    #[default]
    TexLive,
    /// Shelf's packs (`crate::shelf`).
    Shelf,
}

impl Files {
    /// `texlive` or `shelf`.
    pub fn of(name: &str) -> Result<Files, String> {
        match name {
            "texlive" => Ok(Files::TexLive),
            "shelf" => Ok(Files::Shelf),
            n => Err(format!("files = \"{n}\": texlive or shelf")),
        }
    }
}

/// The cache directory: `PHITEX_CACHE_DIR`, else `$XDG_CACHE_HOME/phitex`,
/// else `~/.cache/phitex`, else the temporary directory's.
pub fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PHITEX_CACHE_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("phitex");
    }
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty() && Path::new(h).is_dir()) {
        return PathBuf::from(h).join(".cache").join("phitex");
    }
    std::env::temp_dir().join("phitex-cache")
}

/// TeX Live for one program name: the finder a build is given.
pub struct Tree {
    /// kpathsea as the engine's program (its `progname`, `$engine`).
    kpse: Mutex<Kpse>,
    /// The glyph settings kpathsea was last given (`kpse_init_prog`).
    glyph: Mutex<Option<(u32, Option<String>)>>,
}

impl Tree {
    /// The tree for engine `e`: kpathsea as its program, made once (its
    /// `texmf.cnf` and `ls-R` read at the first lookup).
    pub fn open(e: &Engine) -> Tree {
        Tree {
            kpse: Mutex::new(kpse(e.name, e.kpse_engine)),
            glyph: Mutex::new(None),
        }
    }

    /// The tree a format of engine `e` is built with: its search paths
    /// without the current directory, as fmtutil runs the build in a
    /// scratch directory (a document named `plain.tex` is not the
    /// format's `plain.tex`).
    pub fn open_format(e: &Engine) -> Tree {
        let mut k = kpse(e.name, e.kpse_engine);
        for f in FORMATS {
            k.drop_cwd(f);
        }
        Tree {
            kpse: Mutex::new(k),
            glyph: Mutex::new(None),
        }
    }

    /// Where file `name` is, by kpathsea in the format its extension
    /// names (pdfTeX's choices: a TFM, a VF, a map, a Type 1 font, an
    /// encoding, else an input).
    pub fn input(&self, name: &str) -> Option<PathBuf> {
        if let Some((font, dpi)) = phitex_kpse::pk_name(name) {
            return self.pk(font, dpi, phitex_kpse::pk_settings(name));
        }
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let format = match ext.to_ascii_lowercase().as_str() {
            "tfm" => Format::Tfm,
            "vf" => Format::Vf,
            "map" => Format::FontMap,
            "pfb" | "pfa" => Format::Type1,
            "enc" => Format::Enc,
            "otf" => Format::OpenType,
            "ttf" | "ttc" => Format::TrueType,
            _ => Format::Tex,
        };
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        let found = k
            .find_file(name.as_bytes(), format, false)
            .map(|b| os(&b).to_path_buf());
        drop(k);
        // (kpathsea's mktextfm on a TFM it finds none of)
        match (found, format) {
            (None, Format::Tfm) => {
                crate::mkfont::mktextfm(name.strip_suffix(".tfm").unwrap_or(name))
            }
            (found, _) => found,
        }
    }
}

impl Tree {
    /// Where font `font`'s PK file at `dpi` dots per inch is (the name
    /// `cmr10.600pk` a PK font's lookup asks for, [`phitex_kpse::pk_name`]):
    /// kpathsea's PK search, `kpse_find_pk` ([`Kpse::find_pk`]) with the
    /// glyph settings `glyph` the writer gave (`kpse_init_prog`'s base
    /// resolution and mode, [`phitex_kpse::pk_settings`]), and on a miss
    /// mktexpk ([`crate::mkfont::mktexpk`]).
    pub fn pk(&self, font: &str, dpi: u32, glyph: Option<(u32, Option<&str>)>) -> Option<PathBuf> {
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((bdpi, mode)) = glyph {
            let mut g = self.glyph.lock().unwrap_or_else(PoisonError::into_inner);
            let want = (bdpi, mode.map(str::to_string));
            if g.as_ref() != Some(&want) {
                k.init_prog(bdpi, mode.map(str::as_bytes));
                *g = Some(want);
            }
        }
        let found = k
            .find_pk(font.as_bytes(), dpi)
            .map(|(p, _)| os(&p).to_path_buf());
        drop(k);
        match (found, glyph) {
            (None, Some((bdpi, mode))) => crate::mkfont::mktexpk(font, dpi, bdpi, mode),
            (found, _) => found,
        }
    }

    /// Where a tool's support file `name` is, by kpathsea in the format
    /// of its search (`bst`, `bib`, `ist`, which add their suffixes).
    pub fn tool_input(&self, search: phitex_tex::ToolSearch, name: &str) -> Option<PathBuf> {
        let format = match search {
            phitex_tex::ToolSearch::BibStyle => Format::Bst,
            phitex_tex::ToolSearch::BibData => Format::Bib,
            phitex_tex::ToolSearch::IndexStyle => Format::Ist,
            // (Ghostscript's own fonts, where the system's gs keeps them,
            // as TeX Live's epstopdf runs the system's gs)
            phitex_tex::ToolSearch::GsFont => return gs_font(name),
        };
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        let found = k
            .find_file(name.as_bytes(), format, false)
            .map(|b| os(&b).to_path_buf());
        drop(k);
        // (kpathsea's mktextfm on a TFM it finds none of)
        match (found, format) {
            (None, Format::Tfm) => {
                crate::mkfont::mktextfm(name.strip_suffix(".tfm").unwrap_or(name))
            }
            (found, _) => found,
        }
    }
}

/// A font of the system Ghostscript's `Resource/Font` by its file name.
pub fn gs_font(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return None;
    }
    GS_FONT_DIRS
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

/// Where a system's Ghostscript keeps its `Resource/Font`.
const GS_FONT_DIRS: [&str; 3] = [
    "/usr/share/ghostscript/Resource/Font",
    "/usr/share/ghostscript/10.08.0/Resource/Font",
    "/usr/local/share/ghostscript/Resource/Font",
];

/// The formats [`Tree::input`] looks files up in.
const FORMATS: [Format; 8] = [
    Format::Tfm,
    Format::Vf,
    Format::FontMap,
    Format::Type1,
    Format::Enc,
    Format::OpenType,
    Format::TrueType,
    Format::Tex,
];

pub(crate) fn os(b: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(b))
}
