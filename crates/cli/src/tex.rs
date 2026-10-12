//! TeX Live as the builds see it: the engines by their program names
//! (fmtutil.cnf's lines), and kpathsea, the finder a `Build` is given:
//! `\input` files by TEXINPUTS (from the environment, `.` first as
//! texmf.cnf has it), TFM files, map files, fonts and encodings, each a
//! lookup in the `ls-R` databases, no directory walked.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use phitex_kpse::{Format, Kpse};

/// What a format writes when no `\pdfoutput` says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Dvi,
    Pdf,
}

impl Output {
    pub fn ext(self) -> &'static str {
        match self {
            Output::Dvi => "dvi",
            Output::Pdf => "pdf",
        }
    }
}

/// An engine by its program name, and the format it loads (TeX Live's
/// fmtutil.cnf line for it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Engine {
    /// The program name: kpathsea's `progname`, and the format's name.
    pub name: &'static str,
    /// The format's ini file.
    pub ini: &'static str,
    /// e-TeX's extended mode (the `*` before the ini file).
    pub etex: bool,
    /// kpathsea's `$engine`.
    pub kpse_engine: &'static str,
    /// What the format writes.
    pub output: Output,
}

/// fmtutil.cnf: `tex tex - tex.ini`, `etex pdftex language.def … *etex.ini`,
/// `pdftex pdftex language.def … *pdfetex.ini`, `latex pdftex language.dat
/// … *latex.ini`, `pdflatex pdftex language.dat … *pdflatex.ini`.
const ENGINES: [Engine; 5] = [
    Engine {
        name: "tex",
        ini: "tex.ini",
        etex: false,
        kpse_engine: "tex",
        output: Output::Dvi,
    },
    Engine {
        name: "etex",
        ini: "etex.ini",
        etex: true,
        kpse_engine: "pdftex",
        output: Output::Dvi,
    },
    Engine {
        name: "pdftex",
        ini: "pdfetex.ini",
        etex: true,
        kpse_engine: "pdftex",
        output: Output::Pdf,
    },
    Engine {
        name: "latex",
        ini: "latex.ini",
        etex: true,
        kpse_engine: "pdftex",
        output: Output::Dvi,
    },
    Engine {
        name: "pdflatex",
        ini: "pdflatex.ini",
        etex: true,
        kpse_engine: "pdftex",
        output: Output::Pdf,
    },
];

/// The engine a program name (or a magic comment's, `phitex.toml`'s
/// `engine`) stands for.
pub fn engine(name: &str) -> Result<Engine, String> {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "xetex" | "xelatex" | "xelatex-dev" | "luatex" | "lualatex" | "luahbtex"
        | "lualatex-dev" | "dvilualatex" => Err(format!(
            "{n} is not supported yet: phitex runs pdflatex, latex, pdftex, etex and tex \
             (there is no external TeX compiler)"
        )),
        "pdfetex" => engine("pdftex"),
        "initex" | "virtex" => engine("tex"),
        _ => ENGINES
            .iter()
            .find(|e| e.name == n)
            .copied()
            .ok_or_else(|| format!("unknown engine `{name}`")),
    }
}

impl Engine {
    /// The same kind of document written as `out`: LaTeX's formats are
    /// `latex` (DVI) and `pdflatex`, plain's `etex` and `pdftex` (pdfTeX's
    /// `-output-format` given to a format of the other kind).
    pub fn with_output(self, out: Output) -> Result<Engine, String> {
        if self.output == out {
            return Ok(self);
        }
        match (self.name, out) {
            ("latex", Output::Pdf) => engine("pdflatex"),
            ("pdflatex", Output::Dvi) => engine("latex"),
            ("etex", Output::Pdf) => engine("pdftex"),
            ("pdftex", Output::Dvi) => engine("etex"),
            _ => Err(format!("{} cannot write {}", self.name, out.ext())),
        }
    }
}

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

/// FNV-1a, 64 bits: the stamps' and the cache's keys (stable across
/// runs and builds).
#[derive(Clone, Copy)]
pub struct Fnv(pub u64);

impl Fnv {
    pub fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    /// `bytes` added, then a separator.
    pub fn add(mut self, bytes: &[u8]) -> Fnv {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        self
    }
}

impl Default for Fnv {
    fn default() -> Self {
        Fnv::new()
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
}

impl Tree {
    /// The tree for engine `e`: kpathsea as its program, made once (its
    /// `texmf.cnf` and `ls-R` read at the first lookup).
    pub fn open(e: &Engine) -> Tree {
        Tree {
            kpse: Mutex::new(kpse(e.name, e.kpse_engine)),
        }
    }

    /// Where file `name` is, by kpathsea in the format its extension
    /// names (pdfTeX's choices: a TFM, a VF, a map, a Type 1 font, an
    /// encoding, else an input).
    pub fn input(&self, name: &str) -> Option<PathBuf> {
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
        k.find_file(name.as_bytes(), format, false)
            .map(|b| os(&b).to_path_buf())
    }
}

fn os(b: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(b))
}

/// Bytes as TeX's characters, one each (what a `Build` reads).
pub fn chars(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// TeX's characters as bytes (a character past 255 as `?`).
pub fn bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines() {
        assert_eq!(engine("pdfLaTeX").unwrap().ini, "pdflatex.ini");
        assert_eq!(
            engine("latex")
                .unwrap()
                .with_output(Output::Pdf)
                .unwrap()
                .name,
            "pdflatex"
        );
        assert_eq!(
            engine("pdftex")
                .unwrap()
                .with_output(Output::Dvi)
                .unwrap()
                .name,
            "etex"
        );
        assert!(engine("tex").unwrap().with_output(Output::Pdf).is_err());
        for n in ["XeLaTeX", "xetex", "lualatex", "luatex"] {
            assert!(engine(n).unwrap_err().contains("not supported"), "{n}");
        }
        assert!(engine("foo").is_err());
    }

    #[test]
    fn latin1() {
        let s = chars(&[0x41, 0xc3, 0xa9]);
        assert_eq!(s.chars().count(), 3);
        assert_eq!(bytes(&s), vec![0x41, 0xc3, 0xa9]);
    }
}
