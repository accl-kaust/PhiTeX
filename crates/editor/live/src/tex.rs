//! TeX Live's engines by their program names (fmtutil.cnf's lines), the
//! hashes the session keys its files by, and TeX's characters as bytes:
//! what a session needs of them without TeX Live itself (kpathsea, the
//! files, stay the host's: the CLI's `tex.rs`).

/// What a format writes when no `\pdfoutput` says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Dvi,
    Pdf,
    /// XeTeX's XDV file, made a PDF as xelatex has xdvipdfmx make it
    /// (in process: `dvipdf::xdv_to_pdf`).
    Xdv,
    /// XeTeX's XDV file written as it is (`xelatex -no-pdf`).
    XdvFile,
}

impl Output {
    /// The written file's extension (an XDV is written as its PDF).
    pub fn ext(self) -> &'static str {
        match self {
            Output::Dvi => "dvi",
            Output::Pdf | Output::Xdv => "pdf",
            Output::XdvFile => "xdv",
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

impl Engine {
    /// XeTeX's: native fonts, an XDV file.
    pub fn xetex(&self) -> bool {
        self.kpse_engine == "xetex"
    }
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
        "xelatex-dev" | "luatex" | "lualatex" | "luahbtex" | "lualatex-dev" | "dvilualatex" => {
            Err(format!(
                "{n} is not supported yet: phitex runs pdflatex, latex, xelatex, pdftex, xetex, \
             etex and tex (there is no external TeX compiler)"
            ))
        }
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
        // (XeTeX's XDV is written as its PDF, or as it is with -no-pdf)
        if self.output == out || (self.output == Output::Xdv && out == Output::Pdf) {
            return Ok(self);
        }
        if self.output == Output::Xdv && out == Output::XdvFile {
            return Ok(Engine { output: out, ..self });
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
pub fn samples(len: usize) -> Vec<(usize, usize)> {
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
pub fn words_hash(b: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut h = Fnv::new().0 ^ (b.len() as u64).wrapping_mul(K);
    let (words, tail) = b.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(K).rotate_left(29);
    }
    Fnv(h).add(tail).0
}

impl Default for Fnv {
    fn default() -> Self {
        Fnv::new()
    }
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
