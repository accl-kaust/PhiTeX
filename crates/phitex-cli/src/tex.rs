//! TeX Live as the flow builds see it: the engines by their program names
//! (fmtutil.cnf's lines), kpathsea for the paths, and the directories a
//! `Build` is given.
//!
//! A `Build` finds `\input` files in a list of directories, trying each in
//! order (`Build::set_input_dirs`). kpathsea's TEXINPUTS covers thousands
//! of directories (`//`), so they are folded into one: a directory of
//! links, one per file name, to the file kpathsea's search order finds
//! first ([`Tree::inputs`]). It is made once per program name and TeX
//! Live tree (its `ls-R` databases' stamps) in the cache directory, as
//! `mktexlsr` makes `ls-R`: a cache of the tree, never of a document.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use partex_kpse::{Format, Kpse};

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

/// This executable's identity (its path, size and time): a format made by
/// another build of phitex is never read back.
pub fn exe_stamp() -> u64 {
    let Ok(exe) = std::env::current_exe() else { return 0 };
    let meta = std::fs::metadata(&exe).ok();
    let time = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Fnv::new()
        .add(exe.as_os_str().as_bytes())
        .add(&meta.map_or(0, |m| m.len()).to_le_bytes())
        .add(&time.to_le_bytes())
        .0
}

/// TeX Live for one program name: where a build finds its inputs and its
/// fonts.
pub struct Tree {
    /// The directory of links standing for TEXINPUTS (but `.`).
    pub inputs: PathBuf,
    /// The directories `\font` finds TFM files below.
    pub fonts: Vec<PathBuf>,
    /// The tree's stamp: the search paths and the `ls-R` databases'
    /// sizes and times.
    pub stamp: u64,
}

fn os(b: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(b))
}

impl Tree {
    /// The tree for engine `e` (its links made if the cache has none);
    /// `note` hears what takes time.
    pub fn open(e: &Engine, note: &mut dyn FnMut(&str)) -> Result<Tree, String> {
        let mut kpse = kpse(e.name, e.kpse_engine);
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
        let stamp = h.0;
        let inputs = cache_dir().join("inputs").join(format!("{}-{stamp:016x}", e.name));
        if !inputs.is_dir() {
            let t = std::time::Instant::now();
            let dirs = kpse.search_dirs(Format::Tex);
            let n = link_farm(&dirs, &inputs)
                .map_err(|err| format!("can't make {}: {err}", inputs.display()))?;
            note(&format!(
                "TeX Live's inputs for {}: {n} files of {} directories, linked in {:.1} s ({})",
                e.name,
                dirs.len(),
                t.elapsed().as_secs_f64(),
                inputs.display()
            ));
        }
        let mut fonts = Vec::new();
        for elt in tfm_path.split(|&c| c == b':') {
            let elt = elt.strip_prefix(b"!!").unwrap_or(elt);
            let mut d = elt.to_vec();
            while d.ends_with(b"/") && d.len() > 1 {
                d.pop();
            }
            let p = os(&d).to_path_buf();
            if p.is_absolute() && p.is_dir() && !fonts.contains(&p) {
                fonts.push(p);
            }
        }
        if fonts.is_empty() {
            fonts = phitex_flow::tfm::default_dirs();
        }
        Ok(Tree { inputs, fonts, stamp })
    }

    /// The path of TeX Live's file `name` (as TEXINPUTS finds it), if any.
    pub fn input(&self, name: &str) -> Option<PathBuf> {
        let p = self.inputs.join(name);
        p.is_file().then_some(p)
    }
}

/// Makes `at`: a link per file name in `dirs` (absolute ones, in order),
/// to the first file of that name. Made beside it and renamed, so a
/// process that finds it finds it whole. The number of links.
fn link_farm(dirs: &[Vec<u8>], at: &Path) -> std::io::Result<usize> {
    let parent = at.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}",
        at.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let mut seen = HashSet::new();
    for d in dirs {
        let d = os(d);
        if !d.is_absolute() {
            // (`.`: the document's own directory comes first anyway)
            continue;
        }
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut names: Vec<std::ffi::OsString> = rd
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name())
            .collect();
        names.sort();
        for n in names {
            if seen.insert(n.clone()) {
                std::os::unix::fs::symlink(d.join(&n), tmp.join(&n))?;
            }
        }
    }
    match std::fs::rename(&tmp, at) {
        Ok(()) => Ok(seen.len()),
        // (another process made it meanwhile)
        Err(_) if at.is_dir() => {
            let _ = std::fs::remove_dir_all(&tmp);
            Ok(seen.len())
        }
        Err(e) => Err(e),
    }
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
