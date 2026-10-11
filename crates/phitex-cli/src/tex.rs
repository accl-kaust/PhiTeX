//! TeX Live as the flow builds see it: the engines by their program names
//! (fmtutil.cnf's lines), and kpathsea, the finder a `Build` is given
//! (`Build::set_kpse` with `phitex_flow::kpse::Kpathsea`): `\input` files
//! by TEXINPUTS (from the environment, `.` first as texmf.cnf has it),
//! TFM files, map files, fonts and encodings, each a lookup in the `ls-R`
//! databases, no directory walked.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use partex_kpse::{Format, Kpse};
use phitex_flow::kpse::Kpathsea;
use phitex_flow::tfm::{Kpse as _, KpseFormat};

/// What a format writes when no `\pdfoutput` says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Dvi,
    Pdf,
    /// XeTeX's `-no-pdf`: the XDV file, not given to xdvipdfmx.
    Xdv,
}

impl Output {
    pub fn ext(self) -> &'static str {
        match self {
            Output::Dvi => "dvi",
            Output::Pdf => "pdf",
            Output::Xdv => "xdv",
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
    /// XeTeX: native fonts, XDV pages given to xdvipdfmx (in process).
    pub xetex: bool,
}

/// fmtutil.cnf: `tex tex - tex.ini`, `etex pdftex language.def … *etex.ini`,
/// `pdftex pdftex language.def … *pdfetex.ini`, `latex pdftex language.dat
/// … *latex.ini`, `pdflatex pdftex language.dat … *pdflatex.ini`,
/// `xetex xetex language.def -etex xetex.ini`, `xelatex xetex
/// language.dat -etex xelatex.ini`.
const ENGINES: [Engine; 7] = [
    Engine { name: "tex", ini: "tex.ini", etex: false, kpse_engine: "tex", output: Output::Dvi, xetex: false },
    Engine { name: "etex", ini: "etex.ini", etex: true, kpse_engine: "pdftex", output: Output::Dvi, xetex: false },
    Engine { name: "pdftex", ini: "pdfetex.ini", etex: true, kpse_engine: "pdftex", output: Output::Pdf, xetex: false },
    Engine { name: "latex", ini: "latex.ini", etex: true, kpse_engine: "pdftex", output: Output::Dvi, xetex: false },
    Engine { name: "pdflatex", ini: "pdflatex.ini", etex: true, kpse_engine: "pdftex", output: Output::Pdf, xetex: false },
    Engine { name: "xetex", ini: "xetex.ini", etex: true, kpse_engine: "xetex", output: Output::Pdf, xetex: true },
    Engine { name: "xelatex", ini: "xelatex.ini", etex: true, kpse_engine: "xetex", output: Output::Pdf, xetex: true },
];

/// The engine a program name (or a magic comment's, `phitex.toml`'s
/// `engine`) stands for.
pub fn engine(name: &str) -> Result<Engine, String> {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "luatex" | "lualatex" | "luahbtex" | "lualatex-dev" | "dvilualatex" => Err(format!(
            "{n} is not on the graph: `phitex build --engine {n}` runs the system's {n} as an external \
             process (not incremental)"
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
        // (XeTeX's formats write PDF, or with `-no-pdf` the XDV file)
        if self.xetex {
            return match out {
                Output::Xdv | Output::Pdf => Ok(Engine { output: out, ..self }),
                Output::Dvi => Err(format!("{} writes XDV or PDF, not DVI", self.name)),
            };
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
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
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

/// texmfmp.c's `setupboundvariable`: `name`'s value by kpathsea (the
/// environment first, `name.progname` before `name`, then texmf.cnf) as C's
/// `atoi` reads it; a negative value, or 0 where the default is not,
/// keeps the default (web2c's "Bad value" warning).
fn bound_var(k: &mut Kpse, name: &str, dflt: i64) -> i64 {
    let Some(v) = k.var_value(name) else { return dflt };
    let s = v.trim_ascii_start();
    let (neg, digits) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let n = digits
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .fold(0i64, |n, c| n.saturating_mul(10).saturating_add(i64::from(c - b'0')));
    let n = if neg { -n } else { n };
    if n < 0 || (n == 0 && dflt > 0) { dflt } else { n }
}

/// The main memory of a run that loads a format, in words (tex.ch): the
/// format's `mem_top` (its `main_memory`, as the format was made: ours are
/// made with [`phitex_flow::build::MAIN_MEMORY`], TeX Live's texmf.cnf
/// value; a `main_memory` given to the run is ignored, as pdflatex ignores
/// it), and the run's own `extra_mem_top` above and `extra_mem_bot` below
/// (`mem_max = mem_top + extra_mem_top`, `mem_min = mem_bot -
/// extra_mem_bot`), each at most `sup_main_memory`. An `-ini` run has
/// `main_memory` only (the `Options` default).
pub fn run_memory(k: &mut Kpse) -> u64 {
    const SUP_MAIN_MEMORY: i64 = 256_000_000;
    let main = i64::try_from(phitex_flow::build::MAIN_MEMORY).unwrap_or(SUP_MAIN_MEMORY);
    let top = bound_var(k, "extra_mem_top", 0).min(SUP_MAIN_MEMORY);
    let bot = bound_var(k, "extra_mem_bot", 0).min(SUP_MAIN_MEMORY);
    u64::try_from(main + top + bot).unwrap_or(0)
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

/// This build's identity: a hash of the sources it was built from
/// (`build.rs`'s `build_id`), the same for the same sources in any
/// checkout and for any copy of the binary: a format made by another
/// build of phitex is never read back, one made by a copy of this one is.
pub fn build_id() -> u64 {
    u64::from_str_radix(env!("PHITEX_BUILD_ID"), 16).unwrap_or(0)
}

/// TeX Live for one program name: the finder a build is given, and the
/// tree's stamp.
pub struct Tree {
    /// kpathsea as the engine's program (its `progname`, `$engine`).
    pub kpse: Arc<Kpathsea>,
    /// The program name it finds files as.
    pub prog: &'static str,
    /// The tree's stamp: the search paths and the `ls-R` databases'
    /// sizes and times.
    pub stamp: u64,
    /// The main memory a run of a format has ([`run_memory`]).
    pub main_memory: u64,
}

fn os(b: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(b))
}

impl Tree {
    /// The tree for engine `e`: kpathsea as its program, made once (its
    /// `texmf.cnf` and `ls-R` read at the first lookup).
    pub fn open(e: &Engine) -> Tree {
        let mut k = kpse(e.name, e.kpse_engine);
        let stamp = stamp(&mut k);
        let main_memory = run_memory(&mut k);
        Tree { kpse: Arc::new(Kpathsea::new(k)), prog: e.name, stamp, main_memory }
    }

    /// The same tree for a format's build: the current directory is never
    /// searched (fmtutil runs in a scratch directory), so a format cached
    /// for the tree holds nothing of the directory it was made in.
    pub fn for_format(&self, e: &Engine) -> Tree {
        Tree {
            kpse: Arc::new(Kpathsea::new(kpse(e.name, e.kpse_engine)).without_cwd()),
            prog: self.prog,
            stamp: self.stamp,
            main_memory: self.main_memory,
        }
    }

    /// The path of TeX Live's file `name` (as TEXINPUTS finds it), if any.
    pub fn input(&self, name: &str) -> Option<PathBuf> {
        self.kpse.find(name, KpseFormat::Tex, self.prog).map(PathBuf::from)
    }
}

/// The tree's stamp: the search paths of inputs and TFM files and the
/// `ls-R` databases' sizes and times (a format is made again when the
/// tree changes).
fn stamp(kpse: &mut Kpse) -> u64 {
    let tex_path = kpse.show_path(Format::Tex);
    let tfm_path = kpse.show_path(Format::Tfm);
    let mut h = Fnv::new().add(&tex_path).add(&tfm_path);
    for elt in kpse.show_path(Format::Db).split(|&c| c == b':') {
        let elt = elt.strip_prefix(b"!!").unwrap_or(elt);
        if let Ok(m) = std::fs::metadata(os(elt).join("ls-R")) {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            h = h.add(elt).add(&m.len().to_le_bytes()).add(&t.to_le_bytes());
        }
    }
    h.0
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
        assert_eq!(engine("latex").unwrap().with_output(Output::Pdf).unwrap().name, "pdflatex");
        assert_eq!(engine("pdftex").unwrap().with_output(Output::Dvi).unwrap().name, "etex");
        assert!(engine("tex").unwrap().with_output(Output::Pdf).is_err());
        let x = engine("XeLaTeX").unwrap();
        assert!(x.xetex && x.ini == "xelatex.ini" && x.output == Output::Pdf);
        assert_eq!(x.with_output(Output::Xdv).unwrap().output.ext(), "xdv");
        assert!(x.with_output(Output::Dvi).is_err());
        assert!(engine("foo").is_err());
    }

    #[test]
    fn latin1() {
        let s = chars(&[0x41, 0xc3, 0xa9]);
        assert_eq!(s.chars().count(), 3);
        assert_eq!(bytes(&s), vec![0x41, 0xc3, 0xa9]);
    }
}
