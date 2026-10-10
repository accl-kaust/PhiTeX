//! A font from its TFM file: a pure function of the file's bytes and the
//! size it is loaded at (tex.web part 30, §560–§576, copied as a pure
//! function): dimensions, italic corrections, the ligature/kern program,
//! extensible recipes and parameters.
//!
//! A font's identity is its name and its size (`Font::key`): two loads of
//! the same name at the same size are the same font, an equal value. (TeX's
//! internal font numbers, the order of first loading, are the DVI file's
//! font numbers only: `dvi::numbers`.)

use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CharDims {
    pub w: i32,
    pub h: i32,
    pub d: i32,
}

/// §544: what the remainder of a character's `char_info` means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tag {
    #[default]
    None,
    /// Its ligature/kern program starts at this index.
    Lig(u16),
    /// The next larger character.
    List(u8),
    /// An extensible recipe.
    Ext(u8),
}

/// One existing character's metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Glyph {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub italic: i32,
    pub tag: Tag,
}

/// §545: one lig/kern instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LigKern {
    pub skip: u8,
    pub next: u8,
    pub op: u8,
    pub remainder: u8,
}

impl LigKern {
    /// §545: `stop_flag`, `kern_flag`.
    pub const STOP: u8 = 128;
    pub const KERN: u8 = 128;
}

/// §546: an extensible recipe: top, middle, bottom (0: none) and the
/// repeated piece.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Exten {
    pub top: u8,
    pub mid: u8,
    pub bot: u8,
    pub rep: u8,
}

/// A loaded font. Its tables are shared: a copy is a few counts (a piece's
/// reader holds its own copy of each font it sets, `graph.rs`), and it is
/// laid out apart from its count's cache line (which every copy of the
/// `Arc` writes).
#[derive(Clone)]
#[repr(align(128))]
pub struct Font {
    pub name: String,
    /// The font's identity, its name and size (`cmr10 655360`): what the
    /// `Key`s of its parameters name it by.
    pub key: Arc<str>,
    pub checksum: u32,
    pub design_size: i32,
    pub size: i32,
    pub bc: i32,
    pub ec: i32,
    glyphs: Arc<[Option<Glyph>]>,
    pub lig_kerns: Arc<[LigKern]>,
    pub kerns: Arc<[i32]>,
    pub extens: Arc<[Exten]>,
    /// `params[k]` is `\fontdimen k` as loaded (index 0 unused; at least
    /// seven parameters).
    pub params: Arc<[i32]>,
    /// §545: the right boundary character (256 if none).
    pub bchar: i32,
    /// §549: `bchar` unless that character exists (then 256).
    pub false_bchar: i32,
    /// §545: where the left boundary program starts, if any.
    pub bchar_label: Option<u16>,
    /// The file's bytes (to load it at another size).
    pub data: Arc<[u8]>,
    /// XeTeX: a native font's face and what loading it computed
    /// (`native::NativeFont`); `None` for a TFM font.
    pub native: Option<Arc<crate::native::NativeFont>>,
}

/// A TFM file tex.web would not load (§560's `abort`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadTfm;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<i32, BadTfm> {
        let b = *self.data.get(self.pos).ok_or(BadTfm)?;
        self.pos += 1;
        Ok(i32::from(b))
    }
    fn four(&mut self) -> Result<[u8; 4], BadTfm> {
        let w = self.data.get(self.pos..self.pos + 4).ok_or(BadTfm)?;
        self.pos += 4;
        Ok([w[0], w[1], w[2], w[3]])
    }
    /// §564: `read_sixteen`, rejecting values above 2^15.
    fn sixteen(&mut self) -> Result<i32, BadTfm> {
        let x = self.byte()?;
        if x > 127 {
            return Err(BadTfm);
        }
        Ok(x * 0o400 + self.byte()?)
    }
}

/// §571–§572: the scaling of `fix_word`s by size `z`.
struct Scaler {
    z: i32,
    alpha: i32,
    beta: i32,
}

impl Scaler {
    fn new(mut z: i32) -> Self {
        let mut alpha = 16;
        while z >= 0o40_000_000 {
            z /= 2;
            alpha += alpha;
        }
        let beta = 256 / alpha;
        Self { z, alpha: alpha * z, beta }
    }
    fn scale(&self, [a, b, c, d]: [u8; 4]) -> Result<i32, BadTfm> {
        let (b, c, d) = (i32::from(b), i32::from(c), i32::from(d));
        let z = self.z;
        let sw = ((((d * z) / 0o400) + (c * z)) / 0o400 + (b * z)) / self.beta;
        match a {
            0 => Ok(sw),
            255 => Ok(sw - self.alpha),
            _ => Err(BadTfm),
        }
    }
    fn table(&self, r: &mut Reader<'_>, n: i32) -> Result<Vec<i32>, BadTfm> {
        (0..n).map(|_| self.scale(r.four()?)).collect()
    }
}

impl Font {
    /// §552: the null font: no characters, seven zero parameters.
    pub fn null() -> Font {
        Font {
            name: "nullfont".into(),
            key: Arc::from("nullfont"),
            checksum: 0,
            design_size: 0,
            size: 0,
            bc: 1,
            ec: 0,
            glyphs: Arc::from([]),
            lig_kerns: Arc::from([]),
            kerns: Arc::from([]),
            extens: Arc::from([]),
            params: Arc::from([0; 8]),
            bchar: 256,
            false_bchar: 256,
            bchar_label: None,
            data: Arc::from([]),
            native: None,
        }
    }

    /// Parses a TFM file at its design size.
    pub fn parse(name: &str, b: &[u8]) -> Result<Font, String> {
        Font::load(name, Arc::from(b), None).map_err(|_| format!("{name}: bad TFM file"))
    }

    /// §568: the design size of a TFM file whose size fields are valid.
    pub fn design_size_of(data: &[u8]) -> Result<i32, BadTfm> {
        let mut r = Reader { data, pos: 2 };
        if r.sixteen()? < 2 {
            return Err(BadTfm);
        }
        r.pos = 28;
        let mut z = r.sixteen()?;
        z = z * 0o400 + r.byte()?;
        z = z * 0o20 + r.byte()? / 0o20;
        if z < 0o200_000 {
            return Err(BadTfm);
        }
        Ok(z)
    }

    /// Loads a TFM file at size `at` (`None`: its design size), tex.web
    /// 560–576.
    pub fn load(name: &str, data: Arc<[u8]>, at: Option<i32>) -> Result<Font, BadTfm> {
        let mut r = Reader { data: &data, pos: 0 };
        let lf = r.sixteen()?;
        let lh = r.sixteen()?;
        let mut bc = r.sixteen()?;
        let mut ec = r.sixteen()?;
        if bc > ec + 1 || ec > 255 {
            return Err(BadTfm);
        }
        if bc > 255 {
            bc = 1;
            ec = 0;
        }
        let nw = r.sixteen()?;
        let nh = r.sixteen()?;
        let nd = r.sixteen()?;
        let ni = r.sixteen()?;
        let nl = r.sixteen()?;
        let nk = r.sixteen()?;
        let ne = r.sixteen()?;
        let np = r.sixteen()?;
        if lf != 6 + lh + (ec - bc + 1) + nw + nh + nd + ni + nl + nk + ne + np {
            return Err(BadTfm);
        }
        if nw == 0 || nh == 0 || nd == 0 || ni == 0 || lh < 2 {
            return Err(BadTfm);
        }
        let check = r.four()?;
        let mut z = r.sixteen()?;
        z = z * 0o400 + r.byte()?;
        z = z * 0o20 + r.byte()? / 0o20;
        if z < 0o200_000 {
            return Err(BadTfm);
        }
        for _ in 2..lh {
            r.four()?;
        }
        let design_size = z;
        let size = at.unwrap_or(design_size);
        let n = usize::try_from(ec - bc + 1).unwrap_or(0);
        // (width, height, depth, italic, tag, remainder)
        let mut info: Vec<[u8; 6]> = Vec::with_capacity(n);
        for k in 0..n {
            let [a, b, c, d] = r.four()?;
            let i = [a, b / 16, b % 16, c / 4, c % 4, d];
            let di = i32::from(d);
            if i32::from(a) >= nw || i32::from(i[1]) >= nh || i32::from(i[2]) >= nd || i32::from(i[3]) >= ni {
                return Err(BadTfm);
            }
            match i[4] {
                1 if di >= nl => return Err(BadTfm),
                3 if di >= ne => return Err(BadTfm),
                2 => {
                    // (§570: a charlist cycle)
                    if di < bc || di > ec {
                        return Err(BadTfm);
                    }
                    let current = k as i32 + bc;
                    let mut d = di;
                    while d < current {
                        let q = info[(d - bc) as usize];
                        if q[4] != 2 {
                            break;
                        }
                        d = i32::from(q[5]);
                    }
                    if d == current {
                        return Err(BadTfm);
                    }
                }
                _ => {}
            }
            info.push(i);
        }
        let scaler = Scaler::new(size);
        let widths = scaler.table(&mut r, nw)?;
        let heights = scaler.table(&mut r, nh)?;
        let depths = scaler.table(&mut r, nd)?;
        let italics = scaler.table(&mut r, ni)?;
        if widths[0] != 0 || heights[0] != 0 || depths[0] != 0 || italics[0] != 0 {
            return Err(BadTfm);
        }
        let exists = |c: i32| c >= bc && c <= ec && info[(c - bc) as usize][0] > 0;
        let mut bch_label = 0o77777;
        let mut bchar = 256;
        let mut lig_kerns = Vec::with_capacity(nl as usize);
        for k in 0..nl {
            let [a, b, c, d] = r.four()?;
            let (ai, bi, ci, di) = (i32::from(a), i32::from(b), i32::from(c), i32::from(d));
            if ai > 128 {
                if 256 * ci + di >= nl {
                    return Err(BadTfm);
                }
                if ai == 255 && k == 0 {
                    bchar = bi;
                }
            } else {
                if bi != bchar && !exists(bi) {
                    return Err(BadTfm);
                }
                if ci < 128 {
                    if !exists(di) {
                        return Err(BadTfm);
                    }
                } else if 256 * (ci - 128) + di >= nk {
                    return Err(BadTfm);
                }
                if ai < 128 && k + ai + 1 >= nl {
                    return Err(BadTfm);
                }
            }
            lig_kerns.push(LigKern { skip: a, next: b, op: c, remainder: d });
        }
        if let Some(last) = lig_kerns.last()
            && last.skip == 255
        {
            bch_label = 256 * i32::from(last.op) + i32::from(last.remainder);
        }
        let kerns = scaler.table(&mut r, nk)?;
        let mut extens = Vec::with_capacity(ne as usize);
        for _ in 0..ne {
            let [a, b, c, d] = r.four()?;
            extens.push(Exten { top: a, mid: b, bot: c, rep: d });
            for piece in [a, b, c] {
                if piece != 0 && !exists(i32::from(piece)) {
                    return Err(BadTfm);
                }
            }
            if !exists(i32::from(d)) {
                return Err(BadTfm);
            }
        }
        let mut params = vec![0; 1];
        for k in 1..=np {
            let w = r.four()?;
            if k == 1 {
                let mut sw = i32::from(w[0]);
                if sw > 127 {
                    sw -= 256;
                }
                sw = sw * 0o400 + i32::from(w[1]);
                sw = sw * 0o400 + i32::from(w[2]);
                params.push(sw * 0o20 + i32::from(w[3]) / 0o20);
            } else {
                params.push(scaler.scale(w)?);
            }
        }
        if params.len() < 8 {
            params.resize(8, 0);
        }
        let false_bchar = if exists(bchar) { 256 } else { bchar };
        let glyphs = info
            .iter()
            .map(|i| {
                (i[0] != 0).then(|| Glyph {
                    width: widths[usize::from(i[0])],
                    height: heights[usize::from(i[1])],
                    depth: depths[usize::from(i[2])],
                    italic: italics[usize::from(i[3])],
                    tag: match i[4] {
                        1 => Tag::Lig(u16::from(i[5])),
                        2 => Tag::List(i[5]),
                        3 => Tag::Ext(i[5]),
                        _ => Tag::None,
                    },
                })
            })
            .collect();
        Ok(Font {
            name: name.to_string(),
            key: Font::key_of(name, size),
            checksum: u32::from_be_bytes(check),
            design_size,
            size,
            bc,
            ec,
            glyphs,
            lig_kerns: lig_kerns.into(),
            kerns: kerns.into(),
            extens: extens.into(),
            params: params.into(),
            bchar,
            false_bchar,
            bchar_label: (bch_label < nl).then_some(bch_label as u16),
            data,
            native: None,
        })
    }

    /// The identity of the font `name` at `size`.
    pub fn key_of(name: &str, size: i32) -> Arc<str> {
        Arc::from(format!("{name} {size}"))
    }

    /// The metrics of character `c`, if the font has it (§554).
    #[inline]
    pub fn glyph(&self, c: i32) -> Option<Glyph> {
        let i = usize::try_from(c.wrapping_sub(self.bc)).ok()?;
        self.glyphs.get(i).copied().flatten()
    }

    pub fn char(&self, c: u32) -> Option<CharDims> {
        self.glyph(c as i32).map(|g| CharDims { w: g.width, h: g.height, d: g.depth })
    }

    /// `\fontdimen n` as loaded (0 beyond the parameters).
    pub fn param(&self, n: usize) -> i32 {
        self.params.get(n).copied().unwrap_or(0)
    }

    /// The number of parameters (`font_params`).
    pub fn nparams(&self) -> usize {
        self.params.len() - 1
    }
}

impl PartialEq for Font {
    fn eq(&self, o: &Self) -> bool {
        std::ptr::eq(self, o) || (self.size == o.size && self.checksum == o.checksum && self.name == o.name)
    }
}

impl Eq for Font {}

impl std::hash::Hash for Font {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.name.hash(h);
        self.size.hash(h);
    }
}

/// The null font, shared (tex.web 552).
pub fn null_font() -> Arc<Font> {
    static NULL: std::sync::OnceLock<Arc<Font>> = std::sync::OnceLock::new();
    NULL.get_or_init(|| Arc::new(Font::null())).clone()
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Font({})", self.key)
    }
}

/// Where TFM files are: file name -> path, from walking the font
/// directories once (a pure function of the file system, which is constant
/// during a build).
#[derive(Default)]
pub struct Index {
    paths: std::collections::HashMap<String, std::path::PathBuf>,
}

impl Index {
    pub fn new(dirs: &[std::path::PathBuf]) -> Index {
        Index::of_kinds(dirs, &["tfm"])
    }

    /// The files below `dirs` whose extension is one of `exts` (none:
    /// every file).
    pub fn of_kinds(dirs: &[std::path::PathBuf], exts: &[&str]) -> Index {
        fn walk(d: &std::path::Path, depth: u32, exts: &[&str], out: &mut std::collections::HashMap<String, std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(d) else { return };
            let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    if depth > 0 {
                        walk(&p, depth - 1, exts, out);
                    }
                } else if let Some(n) = p.file_name().and_then(|n| n.to_str())
                    && (exts.is_empty() || p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.contains(&e)))
                {
                    out.entry(n.to_string()).or_insert(p);
                }
            }
        }
        let mut paths = std::collections::HashMap::new();
        for d in dirs {
            walk(d, 8, exts, &mut paths);
        }
        Index { paths }
    }

    /// The bytes of `name.tfm`.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.paths.get(&format!("{name}.tfm")).and_then(|p| std::fs::read(p).ok())
    }
}

/// TeX Live's TFM directories, where they exist.
pub fn default_dirs() -> Vec<std::path::PathBuf> {
    [
        "/usr/share/texmf-dist/fonts/tfm",
        "/usr/share/texlive/texmf-dist/fonts/tfm",
        "/usr/local/texlive/texmf-dist/fonts/tfm",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .filter(|p| p.is_dir())
    .collect()
}

/// The TFM files a build can load: those given to it by name, then the
/// font directories (searched once, on the first name not given). Reading
/// one is a pure function of its name while the files do not change.
pub struct FontFiles {
    given: Vec<(String, Arc<[u8]>)>,
    dirs: Vec<std::path::PathBuf>,
    index: std::sync::OnceLock<Arc<Index>>,
    cache: std::sync::Mutex<std::collections::HashMap<String, Option<Arc<[u8]>>>>,
}

/// The index of each list of directories, made once per process (a pure
/// function of the file system, which does not change while we run).
fn index_of(dirs: &[std::path::PathBuf]) -> Arc<Index> {
    type Indexes = std::sync::Mutex<Vec<(Vec<std::path::PathBuf>, Arc<Index>)>>;
    static INDEXES: std::sync::OnceLock<Indexes> = std::sync::OnceLock::new();
    let mut all = INDEXES.get_or_init(Indexes::default).lock().expect("font indexes");
    if let Some((_, i)) = all.iter().find(|(d, _)| d == dirs) {
        return i.clone();
    }
    let i = Arc::new(Index::new(dirs));
    all.push((dirs.to_vec(), i.clone()));
    i
}

impl FontFiles {
    pub fn new(given: Vec<(String, Arc<[u8]>)>, dirs: Vec<std::path::PathBuf>) -> FontFiles {
        FontFiles { given, dirs, index: std::sync::OnceLock::new(), cache: std::sync::Mutex::default() }
    }

    /// The bytes of `name.tfm`, if there is such a file.
    pub fn get(&self, name: &str) -> Option<Arc<[u8]>> {
        if let Some((_, d)) = self.given.iter().find(|(n, _)| n == name) {
            return Some(d.clone());
        }
        if let Some(v) = self.cache.lock().expect("font cache").get(name) {
            return v.clone();
        }
        let v = self.index.get_or_init(|| index_of(&self.dirs)).read(name).map(Arc::from);
        self.cache.lock().expect("font cache").insert(name.to_string(), v.clone());
        v
    }
}

/// TeX Live's directories of the files PDF output reads (pdfTeX's map
/// files, Type 1 fonts, encodings), where they exist; kpathsea's
/// `TEXMFVAR` first, as it finds `pdftex.map` there.
pub fn pdf_dirs() -> Vec<std::path::PathBuf> {
    [
        "/var/lib/texmf/fonts/map/pdftex",
        "/usr/local/texlive/texmf-var/fonts/map/pdftex",
        "/usr/share/texmf-dist/fonts/map/pdftex",
        "/usr/share/texlive/texmf-dist/fonts/map/pdftex",
        "/usr/local/texlive/texmf-dist/fonts/map/pdftex",
        "/usr/share/texmf-dist/fonts/type1",
        "/usr/share/texlive/texmf-dist/fonts/type1",
        "/usr/local/texlive/texmf-dist/fonts/type1",
        "/usr/share/texmf-dist/fonts/enc",
        "/usr/share/texlive/texmf-dist/fonts/enc",
        "/usr/local/texlive/texmf-dist/fonts/enc",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .filter(|p| p.is_dir())
    .collect()
}

/// A file PDF output reads (a map file, a Type 1 font, an encoding), by
/// name, below [`pdf_dirs`]: its path and bytes. The index and the bytes
/// are made once per process (a pure function of the file system, which
/// does not change while we run).
pub fn pdf_file(name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
    type Cache = std::sync::Mutex<std::collections::HashMap<String, Option<(Arc<str>, Arc<[u8]>)>>>;
    static INDEX: std::sync::OnceLock<Index> = std::sync::OnceLock::new();
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Cache::default);
    if let Some(v) = cache.lock().expect("pdf file cache").get(name) {
        return v.clone();
    }
    let index = INDEX.get_or_init(|| Index::of_kinds(&pdf_dirs(), &["map", "pfb", "pfa", "enc"]));
    let v = index.paths.get(name).and_then(|p| {
        let b = std::fs::read(p).ok()?;
        Some((Arc::from(p.to_string_lossy().as_ref()), Arc::from(b)))
    });
    cache.lock().expect("pdf file cache").insert(name.to_string(), v.clone());
    v
}

/// Shapers (partex-otf's: HarfRust's plans and a memo of results, a cache
/// that never changes a result) lent to whoever shapes a word: a native
/// word is a pure function of its font, text and direction, and the
/// shaper only saves building a plan again. Kept here with the font file
/// cache: no TeX state.
pub fn with_shaper<R>(f: impl FnOnce(&mut partex_otf::shape::Shaper) -> R) -> R {
    static POOL: std::sync::OnceLock<std::sync::Mutex<Vec<partex_otf::shape::Shaper>>> = std::sync::OnceLock::new();
    let pool = POOL.get_or_init(Default::default);
    let mut sh = pool
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .unwrap_or_default();
    let r = f(&mut sh);
    pool.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(sh);
    r
}

/// kpathsea's lookups as a build is given them: a file of a format by
/// name, the path found, and a file's bytes by path (each a pure function
/// of its argument during a build). XeTeX's native fonts (`[file]`
/// names, `.tec` mappings) and xdvipdfmx (its configuration, maps,
/// encodings, fonts, pictures) ask through it.
pub trait Kpse: Send + Sync {
    /// `kpse_find_file(name, format, 0)` as program `prog` (`xetex`,
    /// `dvipdfmx`, …).
    fn find(&self, name: &str, format: KpseFormat, prog: &str) -> Option<String>;
    /// The bytes of the file at `path`.
    fn read(&self, path: &str) -> Option<Arc<[u8]>>;
}

/// kpathsea's formats asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KpseFormat {
    OpenType,
    TrueType,
    Type1,
    MiscFonts,
    FontMap,
    Cmap,
    Sfd,
    Enc,
    Tfm,
    Ofm,
    Vf,
    Ovf,
    Pict,
    Tex,
    ProgramText,
    ProgramBinary,
}

impl KpseFormat {
    /// The extensions kpathsea tries for a name without one.
    pub fn suffixes(self) -> &'static [&'static str] {
        match self {
            KpseFormat::OpenType => &[".otf", ".OTF"],
            KpseFormat::TrueType => &[".ttf", ".ttc", ".TTF", ".TTC", ".dfont"],
            KpseFormat::Type1 => &[".pfb", ".pfa"],
            KpseFormat::FontMap => &[".map"],
            KpseFormat::Enc => &[".enc"],
            KpseFormat::Tfm => &[".tfm"],
            KpseFormat::Ofm => &[".ofm", ".tfm"],
            KpseFormat::Vf => &[".vf"],
            KpseFormat::Ovf => &[".ovf", ".vf"],
            KpseFormat::Tex => &[".tex"],
            _ => &[],
        }
    }

    /// `kpsewhich -format=` names.
    pub fn name(self) -> &'static str {
        match self {
            KpseFormat::OpenType => "opentype fonts",
            KpseFormat::TrueType => "truetype fonts",
            KpseFormat::Type1 => "type1 fonts",
            KpseFormat::MiscFonts => "misc fonts",
            KpseFormat::FontMap => "map",
            KpseFormat::Cmap => "cmap files",
            KpseFormat::Sfd => "subfont definition files",
            KpseFormat::Enc => "enc files",
            KpseFormat::Tfm => "tfm",
            KpseFormat::Ofm => "ofm",
            KpseFormat::Vf => "vf",
            KpseFormat::Ovf => "ovf",
            KpseFormat::Pict => "graphic/figure",
            KpseFormat::Tex => "tex",
            KpseFormat::ProgramText => "other text files",
            KpseFormat::ProgramBinary => "other binary files",
        }
    }
}

/// [`Kpse`] over directories walked once: a name is found in the first
/// directory (in order, recursively) holding a file of that name, or of
/// that name with one of its format's suffixes. Reads are cached.
pub struct DirKpse {
    index: Index,
    files: std::sync::Mutex<std::collections::HashMap<String, Option<Arc<[u8]>>>>,
}

impl DirKpse {
    pub fn new(dirs: &[std::path::PathBuf]) -> DirKpse {
        DirKpse { index: Index::of_kinds(dirs, &[]), files: std::sync::Mutex::default() }
    }
}

impl Kpse for DirKpse {
    fn find(&self, name: &str, format: KpseFormat, _prog: &str) -> Option<String> {
        if name.starts_with('/') {
            return std::path::Path::new(name).exists().then(|| name.to_string());
        }
        let base = name.rsplit('/').next().unwrap_or(name);
        let try_one = |n: &str| self.index.paths.get(n).map(|p| p.to_string_lossy().into_owned());
        if let Some(p) = try_one(base) {
            return Some(p);
        }
        format.suffixes().iter().find_map(|s| try_one(&format!("{base}{s}")))
    }

    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        let mut g = self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.entry(path.to_string())
            .or_insert_with(|| std::fs::read(path).ok().map(Arc::from))
            .clone()
    }
}

/// [`Kpse`] through TeX Live's `kpsewhich` (`-progname`, `-format`): what
/// kpathsea itself finds, each answer and each file read once (the CLI's
/// and the XeTeX tests' finder; the build never runs it but through this
/// given).
#[derive(Default)]
pub struct Kpsewhich {
    found: std::sync::Mutex<std::collections::HashMap<(String, &'static str, String), Option<String>>>,
    files: std::sync::Mutex<std::collections::HashMap<String, Option<Arc<[u8]>>>>,
}

impl Kpsewhich {
    pub fn new() -> Kpsewhich {
        Kpsewhich::default()
    }
}

impl Kpse for Kpsewhich {
    fn find(&self, name: &str, format: KpseFormat, prog: &str) -> Option<String> {
        if name.starts_with('/') {
            return std::path::Path::new(name).exists().then(|| name.to_string());
        }
        let key = (name.to_string(), format.name(), prog.to_string());
        if let Some(r) = self.found.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return r.clone();
        }
        let r = std::process::Command::new("kpsewhich")
            .arg(format!("-progname={prog}"))
            .arg(format!("-format={}", format.name()))
            .arg(name)
            .output()
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim_end_matches('\n').to_string();
                (o.status.success() && !s.is_empty()).then_some(s)
            });
        self.found.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, r.clone());
        r
    }

    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        let mut g = self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.entry(path.to_string())
            .or_insert_with(|| std::fs::read(path).ok().map(Arc::from))
            .clone()
    }
}

/// The build's given for XeTeX's native fonts: the installed fonts' index
/// (what fontconfig lists, partex-otf's `FontIndex`) and the file finder
/// (kpathsea's `OPENTYPEFONTS`, `TTFONTS`, the mappings), both given, never
/// searched for; each load a pure function of (name, size, tracing),
/// memoised for the build (a fresh `FontManager` per load: no state
/// crosses loads).
pub struct Fonts {
    index: Arc<partex_otf::index::FontIndex>,
    kpse: Arc<dyn Kpse>,
    memo: std::sync::Mutex<std::collections::HashMap<(String, i32, i32), crate::native::Loaded>>,
}

impl Fonts {
    pub fn new(index: Arc<partex_otf::index::FontIndex>, kpse: Arc<dyn Kpse>) -> Fonts {
        Fonts { index, kpse, memo: std::sync::Mutex::default() }
    }

    /// The fonts of an index file (partex-otf's `otf-index` output: the
    /// faces fontconfig lists, in its order), found and read by `kpse`.
    pub fn from_index_bytes(index: &[u8], kpse: Arc<dyn Kpse>) -> Option<Fonts> {
        let idx = partex_otf::index::FontIndex::from_bytes(index)?;
        Some(Fonts::new(Arc::new(idx), kpse))
    }

    /// The finder.
    pub fn kpse(&self) -> &Arc<dyn Kpse> {
        &self.kpse
    }

    /// xetex.web's `load_native_font(name, s)`: the font, and what XeTeX
    /// prints meanwhile (`\XeTeXtracingfonts`).
    pub fn load(&self, name: &str, s: i32, tracing: i32) -> crate::native::Loaded {
        let key = (name.to_string(), s, tracing);
        if let Some(l) = self.memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key) {
            return l.clone();
        }
        let mut mgr = partex_otf::xetex::fontmgr::FontManager::new(self.index.clone());
        let src = Source(&*self.kpse);
        let (xf, diags) = partex_otf::xetex::find_native_font(&mut mgr, &src, name, s, tracing);
        let f = xf.map(|xf| {
            // (`load_native_font`: the size asked, or the design size
            // scaled)
            let actual = if s >= 0 {
                s
            } else if s != -1000 {
                crate::arith::xn_over_d(xf.design_size, -s, 1000).0
            } else {
                xf.design_size
            };
            Arc::new(crate::native::make_font(xf, name, actual))
        });
        let l: crate::native::Loaded = (f, diags);
        self.memo.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, l.clone());
        l
    }
}

/// partex-otf's file source over the build's finder.
struct Source<'a>(&'a dyn Kpse);

impl partex_otf::FontSource for Source<'_> {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        self.0.read(path)
    }

    fn find_file(&self, name: &str, format: partex_otf::KpseFormat) -> Option<String> {
        use KpseFormat as K;
        let f = match format {
            partex_otf::KpseFormat::OpenType => K::OpenType,
            partex_otf::KpseFormat::TrueType => K::TrueType,
            partex_otf::KpseFormat::Type1 => K::Type1,
            partex_otf::KpseFormat::MiscFonts => K::MiscFonts,
        };
        self.0.find(name, f, "xetex")
    }
}

