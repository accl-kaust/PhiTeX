//! TeX Live's kpathsea as a build is given it: [`tfm::Kpse`] over
//! `partex-kpse` (texmf.cnf, the `ls-R` databases, the search paths), so
//! a name becomes a path by a database lookup and no build walks TeX
//! Live's tree. A given of the build, never a node: the bytes of the path
//! found are what a read of `Key::File` (or a font's, a PDF file's) finds,
//! as with the input directories.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use partex_kpse::Format;

use crate::tfm::{self, KpseFormat};

/// kpathsea for a program (`kpathsea_instance`), behind the trait's
/// `&self`: lookups are rare (each name once per build), so one lock.
pub struct Kpathsea {
    k: Mutex<partex_kpse::Kpse>,
    /// The current directory left out of every search path
    /// ([`Kpathsea::without_cwd`]).
    no_cwd: bool,
}

/// kpathsea's format (`kpse_file_format_type`) of each format a build
/// asks for: the same names, one for one.
pub fn format(f: KpseFormat) -> Format {
    match f {
        KpseFormat::OpenType => Format::OpenType,
        KpseFormat::TrueType => Format::TrueType,
        KpseFormat::Type1 => Format::Type1,
        KpseFormat::MiscFonts => Format::MiscFonts,
        KpseFormat::FontMap => Format::FontMap,
        KpseFormat::Cmap => Format::Cmap,
        KpseFormat::Sfd => Format::Sfd,
        KpseFormat::Enc => Format::Enc,
        KpseFormat::Tfm => Format::Tfm,
        KpseFormat::Ofm => Format::Ofm,
        KpseFormat::Vf => Format::Vf,
        KpseFormat::Ovf => Format::Ovf,
        KpseFormat::Pict => Format::Pict,
        KpseFormat::Tex => Format::Tex,
        KpseFormat::ProgramText => Format::ProgramText,
        KpseFormat::ProgramBinary => Format::ProgramBinary,
    }
}

/// The format pdfTeX asks kpathsea for a file PDF output reads, by its
/// name's extension: `pdftex.map` (`kpse_fontmap_format`, mapfile.c), a
/// Type 1 font (`kpse_type1_format`, writet1.c), an encoding
/// (`kpse_enc_format`, writeenc.c), a TrueType or OpenType font
/// (writettf.c), a TFM or VF; anything else as an input (`tex`: images).
pub fn pdf_format(name: &str) -> KpseFormat {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "map" => KpseFormat::FontMap,
        "pfb" | "pfa" => KpseFormat::Type1,
        "enc" => KpseFormat::Enc,
        "ttf" | "ttc" | "dfont" => KpseFormat::TrueType,
        "otf" => KpseFormat::OpenType,
        "tfm" => KpseFormat::Tfm,
        "vf" => KpseFormat::Vf,
        _ => KpseFormat::Tex,
    }
}

/// The program name the default finder asks as.
pub const TEX_LIVE_PROG: &str = "pdftex";

/// TeX Live's kpathsea as `pdftex`, made once per process (its texmf.cnf
/// and `ls-R` read at the first lookup): what a build finds TFM and PDF
/// output's files with by default. None where no TeX Live is found
/// ([`Kpathsea::from_path`]) or `PHITEX_NO_KPSE` is set (not empty): then
/// TeX Live's directories are walked, as before.
pub fn tex_live() -> Option<Arc<Kpathsea>> {
    static TEX_LIVE: std::sync::OnceLock<Option<Arc<Kpathsea>>> = std::sync::OnceLock::new();
    TEX_LIVE
        .get_or_init(|| {
            if std::env::var_os("PHITEX_NO_KPSE").is_some_and(|v| !v.is_empty()) {
                return None;
            }
            Kpathsea::from_path(TEX_LIVE_PROG, TEX_LIVE_PROG).map(Arc::new)
        })
        .clone()
}

impl Kpathsea {
    /// Over an instance made for the program (`Kpse::new(selfdir,
    /// progname, engine)`).
    pub fn new(k: partex_kpse::Kpse) -> Kpathsea {
        Kpathsea { k: Mutex::new(k), no_cwd: false }
    }

    /// The same, never finding a file in the current directory (`.` left
    /// out of every search path): a format's build, which fmtutil runs in
    /// a scratch directory.
    #[must_use]
    pub fn without_cwd(mut self) -> Kpathsea {
        self.no_cwd = true;
        self
    }

    /// kpathsea as TeX Live's programs on `PATH` run it, as program
    /// `progname` of engine `engine`: `SELFAUTOLOC` is the directory of
    /// `kpsewhich` (else `tex`) on `PATH`, links resolved, as web2c finds
    /// its own; none if no `texmf.cnf` is reachable from there.
    pub fn from_path(progname: &str, engine: &str) -> Option<Kpathsea> {
        let dir = bin_dir()?;
        let mut k = partex_kpse::Kpse::new(&dir, progname, engine);
        k.var_value("TEXMF")?;
        Some(Kpathsea::new(k))
    }

    /// The instance, for what the trait does not ask (`show_path`,
    /// `var_value`…).
    pub fn with<R>(&self, f: impl FnOnce(&mut partex_kpse::Kpse) -> R) -> R {
        f(&mut self.k.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The directory of TeX Live's programs on `PATH` (`kpsewhich`, else
/// `tex`), links resolved.
pub fn bin_dir() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    ["kpsewhich", "tex"].iter().find_map(|prog| {
        std::env::split_paths(&path)
            .filter_map(|d| d.join(prog).canonicalize().ok())
            .find(|p| p.is_file())
            .and_then(|p| p.parent().map(Path::to_path_buf))
    })
}

impl tfm::Kpse for Kpathsea {
    /// `kpse_find_file(name, format, false)` as program `prog`, as
    /// `kpsewhich -progname=PROG -format=FORMAT NAME` finds it: the
    /// databases answer for the trees they cover (a name they lack is not
    /// looked for on the disk below them), the disk for the others (`.`).
    fn find(&self, name: &str, format: KpseFormat, prog: &str) -> Option<String> {
        let f = self::format(format);
        self.with(|k| {
            if !prog.is_empty() {
                k.reset_program_name(prog.as_bytes());
            }
            if self.no_cwd {
                k.drop_cwd(f);
            }
            k.find_file(name.as_bytes(), f, false)
        })
        .and_then(|p| String::from_utf8(p).ok())
    }

    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        std::fs::read(path).ok().map(Arc::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_formats() {
        assert_eq!(pdf_format("pdftex.map"), KpseFormat::FontMap);
        assert_eq!(pdf_format("cmr10.pfb"), KpseFormat::Type1);
        assert_eq!(pdf_format("cm-super-t1.enc"), KpseFormat::Enc);
        assert_eq!(pdf_format("a.png"), KpseFormat::Tex);
    }

    /// Each format is kpathsea's of the same name (`kpsewhich -format=`).
    #[test]
    fn formats_one_for_one() {
        let all = [
            KpseFormat::OpenType,
            KpseFormat::TrueType,
            KpseFormat::Type1,
            KpseFormat::MiscFonts,
            KpseFormat::FontMap,
            KpseFormat::Cmap,
            KpseFormat::Sfd,
            KpseFormat::Enc,
            KpseFormat::Tfm,
            KpseFormat::Ofm,
            KpseFormat::Vf,
            KpseFormat::Ovf,
            KpseFormat::Pict,
            KpseFormat::Tex,
            KpseFormat::ProgramText,
            KpseFormat::ProgramBinary,
        ];
        let mut seen = std::collections::HashSet::new();
        for f in all {
            assert!(seen.insert(format(f)), "{f:?} shares kpathsea's format");
        }
    }
}
