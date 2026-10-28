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
    /// XeTeX's XDV file, made a PDF as xelatex has xdvipdfmx make it
    /// (in process: `dvipdf::xdv_to_pdf`).
    Xdv,
}

impl Output {
    /// The written file's extension (an XDV is written as its PDF).
    pub fn ext(self) -> &'static str {
        match self {
            Output::Dvi => "dvi",
            Output::Pdf | Output::Xdv => "pdf",
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
/// … *latex.ini`, `pdflatex pdftex language.dat … *pdflatex.ini`, `xetex
/// xetex language.def -etex xetex.ini`, `xelatex xetex language.dat -etex
/// xelatex.ini`.
const ENGINES: [Engine; 7] = [
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
    Engine {
        name: "xetex",
        ini: "xetex.ini",
        etex: true,
        kpse_engine: "xetex",
        output: Output::Xdv,
    },
    Engine {
        name: "xelatex",
        ini: "xelatex.ini",
        etex: true,
        kpse_engine: "xetex",
        output: Output::Xdv,
    },
];

/// The engine a program name (or a magic comment's, `phitex.toml`'s
/// `engine`) stands for.
pub fn engine(name: &str) -> Result<Engine, String> {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "xelatex-dev" | "luatex" | "lualatex" | "luahbtex" | "lualatex-dev" | "dvilualatex" => Err(format!(
            "{n} is not supported yet: phitex runs pdflatex, latex, xelatex, pdftex, xetex, \
             etex and tex (there is no external TeX compiler)"
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
        // (XeTeX's XDV is written as its PDF)
        if self.output == out || (self.output == Output::Xdv && out == Output::Pdf) {
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

/// The bytes a [`fingerprint`] reads past 1 MB: `(offset, length)`.
fn samples(len: usize) -> Vec<(usize, usize)> {
    const W: usize = 64 << 10;
    let mut v = vec![(0, W)];
    let mut at = 1 << 20;
    while at + 256 < len.saturating_sub(W) {
        v.push((at, 256));
        at += 1 << 20;
    }
    v.push((len - W, W));
    v
}

/// A written file's hash standing for any bytes: a file the watch
/// appends to (its PDF's update sections), whose changes are all its own.
pub const ANY_BYTES: u64 = u64::MAX;

/// The watch's mark of a file it wrote, to know the file as its own when
/// the system says it changed: the bytes' hash below 1 MB; past it the
/// length and samples (its head, its tail, 256 bytes a megabyte), so
/// that neither the write nor the check reads a large output whole.
pub fn fingerprint(b: &[u8]) -> u64 {
    if b.len() <= 1 << 20 {
        return words_hash(b);
    }
    let mut h = Fnv::new().add(&(b.len() as u64).to_le_bytes());
    for (at, n) in samples(b.len()) {
        h = h.add(&b[at..at + n]);
    }
    h.0
}

/// A hash of `b` eight bytes a step (FNV's byte steps took a
/// millisecond on an output of half a megabyte, every build): each word
/// mixed in by a multiply and a rotate, the length and the tail last.
fn words_hash(b: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut h = Fnv::new().0 ^ (b.len() as u64).wrapping_mul(K);
    let (words, tail) = b.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(K).rotate_left(29);
    }
    Fnv(h).add(tail).0
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

impl Tree {
    /// Where a tool's support file `name` is, by kpathsea in the format
    /// of its search (`bst`, `bib`, `ist`, which add their suffixes).
    pub fn tool_input(&self, search: phitex_tex::ToolSearch, name: &str) -> Option<PathBuf> {
        let format = match search {
            phitex_tex::ToolSearch::BibStyle => Format::Bst,
            phitex_tex::ToolSearch::BibData => Format::Bib,
            phitex_tex::ToolSearch::IndexStyle => Format::Ist,
        };
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        k.find_file(name.as_bytes(), format, false)
            .map(|b| os(&b).to_path_buf())
    }
}

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

fn os(b: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(b))
}

/// [`chars`] of bytes given away: ASCII bytes become the text without a
/// copy.
pub fn chars_owned(b: Vec<u8>) -> String {
    if b.is_ascii() {
        return String::from_utf8(b).unwrap_or_default();
    }
    chars(&b)
}

/// Bytes as TeX's characters, one each (what a `Build` reads).
pub fn chars(b: &[u8]) -> String {
    // (ASCII bytes are their characters' UTF-8: copied at once)
    if b.is_ascii() {
        return String::from_utf8(b.to_vec()).unwrap_or_default();
    }
    b.iter().map(|&c| char::from(c)).collect()
}

/// TeX's characters as bytes (a character past 255 as `?`).
pub fn bytes(s: &str) -> Vec<u8> {
    if s.is_ascii() {
        return s.as_bytes().to_vec();
    }
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
        for n in ["lualatex", "luatex"] {
            assert!(engine(n).unwrap_err().contains("not supported"), "{n}");
        }
        assert_eq!(engine("XeLaTeX").unwrap().ini, "xelatex.ini");
        assert_eq!(engine("xetex").unwrap().kpse_engine, "xetex");
        assert_eq!(
            engine("xelatex")
                .unwrap()
                .with_output(Output::Pdf)
                .unwrap()
                .output,
            Output::Xdv
        );
        assert!(engine("foo").is_err());
    }

    #[test]
    fn latin1() {
        let s = chars(&[0x41, 0xc3, 0xa9]);
        assert_eq!(s.chars().count(), 3);
        assert_eq!(bytes(&s), vec![0x41, 0xc3, 0xa9]);
    }
}
