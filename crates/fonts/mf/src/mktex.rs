//! Fonts made as TeX Live makes them on demand: the base as `fmtutil`
//! builds it (`mf-nowin -ini -jobname=mf -translate-file=cp227.tcx
//! mf.ini`), the TFM as `mktextfm` runs METAFONT, the PK as `mktexpk`
//! runs it (the magnification as kpathsea's `tex-make.c` writes
//! `MAKETEX_MAG`) and `gftopk` packs its GF file.

use crate::{Config, FileKind, Io, Mf};

/// A run of METAFONT: its history, terminal output and output files.
pub struct Run {
    pub history: i32,
    pub term: Vec<u8>,
    pub outputs: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Run {
    /// The output file named `name`.
    #[must_use]
    pub fn output(&self, name: &str) -> Option<&[u8]> {
        self.outputs
            .iter()
            .find(|(n, _)| n == name.as_bytes())
            .map(|(_, d)| d.as_slice())
    }
    /// The output file whose name ends in `suffix`.
    #[must_use]
    pub fn output_ending(&self, suffix: &str) -> Option<(&[u8], &[u8])> {
        self.outputs
            .iter()
            .find(|(n, _)| n.ends_with(suffix.as_bytes()))
            .map(|(n, d)| (n.as_slice(), d.as_slice()))
    }
    fn failure(&self, what: &str) -> String {
        format!(
            "{what}: METAFONT failed:\n{}",
            String::from_utf8_lossy(&self.term)
        )
    }
}

/// The tables of `read_tcx_file` (lib/texmfmp.c) applied to the defaults
/// of `mf.web` §21-§23: `(xord, xchr, xprn)`.
#[must_use]
pub fn tcx_tables(tcx: &[u8], eight_bit: bool) -> (Box<[u8; 256]>, Box<[u8; 256]>, Box<[u8; 256]>) {
    let mut xchr = Box::new([0u8; 256]);
    let mut xord = Box::new([0o177u8; 256]);
    let mut xprn = Box::new([0u8; 256]);
    for i in 0..256 {
        xchr[i] = i as u8;
    }
    for i in 0o200..=0o377usize {
        xord[xchr[i] as usize] = i as u8;
    }
    for i in 0..=0o176usize {
        xord[xchr[i] as usize] = i as u8;
    }
    for i in 0..256usize {
        xprn[i] = u8::from(eight_bit || (32..=126).contains(&i));
    }
    for line in tcx.split(|&b| b == b'\n') {
        let line = match line.iter().position(|&b| b == b'%') {
            Some(k) => &line[..k],
            None => line,
        };
        let (first, rest) = tcx_num(line, 255);
        let Some(first) = first else { continue };
        let (second, rest) = tcx_num(rest, 255);
        let (second, printable) = match second {
            Some(second) => {
                xord[first as usize] = second as u8;
                xchr[second as usize] = first as u8;
                let mut printable = tcx_num(rest, 1).0.unwrap_or(1);
                if (32..=126).contains(&second) {
                    printable = 1;
                }
                (second, printable)
            }
            None => (first, 1),
        };
        xprn[second as usize] = printable as u8;
    }
    (xord, xchr, xprn)
}

/// `tcx_get_num`: C's `strtol(s, &end, 0)`, `None` where there is no
/// number or it is outside `0..=upb`, and the rest of the line.
fn tcx_num(s: &[u8], upb: i64) -> (Option<i64>, &[u8]) {
    let mut i = 0;
    while i < s.len() && (s[i] == b' ' || (9..=13).contains(&s[i])) {
        i += 1;
    }
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let (radix, mut j) = if s[i..].starts_with(b"0x") || s[i..].starts_with(b"0X") {
        if s.get(i + 2).is_some_and(u8::is_ascii_hexdigit) {
            (16, i + 2)
        } else {
            (8, i + 1)
        }
    } else if s.get(i) == Some(&b'0') {
        (8, i)
    } else {
        (10, i)
    };
    let start = j;
    let mut n: i64 = 0;
    while let Some(d) = s.get(j).and_then(|&c| (c as char).to_digit(radix)) {
        n = n
            .saturating_mul(i64::from(radix))
            .saturating_add(i64::from(d));
        j += 1;
    }
    if j == start && !(radix == 8 && start > i) {
        return (None, s);
    }
    let n = if neg { -n } else { n };
    if n < 0 || n > upb {
        (None, &s[j..])
    } else {
        (Some(n), &s[j..])
    }
}

/// The files METAFONT reads, and the base it loads.
struct WithBase {
    io: Box<dyn Io>,
    base: Option<(Vec<u8>, String)>,
}

impl Io for WithBase {
    fn open_in(&mut self, name: &[u8], kind: FileKind) -> Option<(Vec<u8>, Vec<u8>)> {
        if kind == FileKind::Base {
            let (b, n) = self.base.as_ref()?;
            return (name == format!("{n}.base").as_bytes()).then(|| (b.clone(), name.to_vec()));
        }
        self.io.open_in(name, kind)
    }
}

/// Settings for the runs of METAFONT a TeX Live installation makes:
/// texmf.cnf's values for `mf` and `inimf`.
#[derive(Clone, Debug)]
pub struct Settings {
    /// `MAKETEX_MODE` (mktex.cnf: `ljfour`).
    pub mode: String,
    /// The date and time (`date_and_time`): minutes after midnight, day,
    /// month, year.
    pub date: (i32, i32, i32, i32),
    /// web2c's version string.
    pub version_string: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: "ljfour".into(),
            date: (22 * 60 + 13, 14, 11, 2023),
            version_string: " (TeX Live 2026)".into(),
        }
    }
}

fn run(cfg: Config, io: Box<dyn Io>, base: Option<(&[u8], &str)>) -> Run {
    let wb = WithBase {
        io,
        base: base.map(|(b, n)| (b.to_vec(), n.to_string())),
    };
    let mut mf = Mf::new(cfg, Box::new(wb));
    let history = mf.run();
    Run {
        history,
        term: std::mem::take(&mut mf.term_out),
        outputs: std::mem::take(&mut mf.outputs),
    }
}

/// The base `mf.base` as fmtutil builds it: INIMF on `mf.ini` with the
/// TCX file `tcx` (its name and bytes; TeX Live's `cp227.tcx`).
///
/// # Errors
/// METAFONT's terminal output where no base was dumped.
pub fn build_base(
    io: Box<dyn Io>,
    tcx: Option<(&str, &[u8])>,
    s: &Settings,
) -> Result<Vec<u8>, String> {
    let translate = tcx.map(|(n, d)| {
        let (o, c, p) = tcx_tables(d, false);
        (n.to_string(), o, c, p)
    });
    let cfg = Config {
        ini: true,
        dump_name: "mf".into(),
        job_name: Some("mf".into()),
        command_line: vec![b"mf.ini".to_vec()],
        translate,
        date: s.date,
        version_string: s.version_string.clone(),
        ..Config::default()
    };
    let r = run(cfg, io, None);
    r.output("mf.base")
        .map(<[u8]>::to_vec)
        .ok_or_else(|| r.failure("mf.base"))
}

/// `kpse_magstep_fix(dpi, bdpi)` (kpathsea `magstep.c`): the resolution
/// and kpathsea's `m_ret` (a comparison, so 0 but where the resolution is
/// the magstep's number).
#[must_use]
pub fn magstep_fix(dpi: i32, bdpi: i32) -> (i32, i32) {
    let sign = if dpi < bdpi { -1 } else { 1 };
    let mut real = 0;
    let mut m = 0;
    let mut mdpi = -1;
    while real == 0 && m < 40 {
        mdpi = magstep(m * sign, bdpi);
        if (mdpi - dpi).abs() <= 1 {
            real = mdpi;
        } else if (mdpi - dpi) * sign > 0 {
            real = dpi;
        }
        m += 1;
    }
    // (`*m_ret = real_dpi == (unsigned) (mdpi ? (m - 1) * sign : 0)`: a
    // comparison, 1 only where the resolution is the magstep's number
    // itself, so MAKETEX_MAG is `DPI/BDPI+DPI%BDPI/BDPI` at magsteps too,
    // as TeX Live's mktexpk is run: `--mag 1+264/600 --dpi 864`, mc job
    // 77344)
    let mret = i32::from(real == if mdpi == 0 { 0 } else { (m - 1) * sign });
    (if real == 0 { dpi } else { real }, mret)
}

#[allow(clippy::cast_possible_truncation)]
fn magstep(n: i32, bdpi: i32) -> i32 {
    let neg = n < 0;
    let mut n = n.abs();
    let mut t = if n & 1 == 1 {
        n &= !1;
        1.095_445_115
    } else {
        1.0
    };
    while n > 8 {
        n -= 8;
        t *= 2.0736;
    }
    while n > 0 {
        n -= 2;
        t *= 1.2;
    }
    let b = f64::from(bdpi);
    (0.5 + if neg { b / t } else { b * t }) as i32
}

/// `MAKETEX_MAG` (kpathsea `tex-make.c` `set_maketex_mag`) for a font at
/// `dpi` of base resolution `bdpi`, as `mktexpk` hands it to METAFONT.
#[must_use]
pub fn maketex_mag(dpi: i32, bdpi: i32) -> String {
    let (_, m) = magstep_fix(dpi, bdpi);
    if m == 0 {
        let (a, b) = (dpi / bdpi, dpi % bdpi);
        if bdpi <= 4000 {
            format!("{a}+{b}/{bdpi}")
        } else {
            let (f, r) = (bdpi / 4000, bdpi % 4000);
            if f > 1 {
                if r > 0 {
                    format!("{a}+{b}/({f}*4000+{r})")
                } else {
                    format!("{a}+{b}/({f}*4000)")
                }
            } else {
                format!("{a}+{b}/(4000+{r})")
            }
        }
    } else {
        let (sign, m) = if m < 0 { ("-", -m) } else { ("", m) };
        format!("magstep({sign}{}.{})", m / 2, (m & 1) * 5)
    }
}

/// mktexpk's guess of a mode from base resolution `bdpi`.
#[must_use]
pub fn guess_mode(bdpi: i32) -> Option<&'static str> {
    Some(match bdpi {
        85 => "sun",
        100 => "nextscrn",
        180 => "toshiba",
        300 => "cx",
        360 => "epstylus",
        400 => "nexthi",
        600 => "ljfour",
        720 => "epscszz",
        1200 => "ultre",
        1270 => "linoone",
        8000 => "dpdfezzz",
        _ => return None,
    })
}

/// The mode mktexpk runs METAFONT in: `mode` (`--mfmode`, else mktex.opt's
/// `ljfour`) where METAFONT's `pixels_per_inch` for it is `bdpi`, else
/// its guess from `bdpi` ([`guess_mode`]).
///
/// # Errors
/// mktexpk's message where it can guess none.
pub fn mktexpk_mode(
    io: Box<dyn Io>,
    base: &[u8],
    mode: Option<&str>,
    bdpi: i32,
    s: &Settings,
) -> Result<String, String> {
    let mode = mode
        .filter(|m| !m.is_empty() && *m != "/")
        .unwrap_or("ljfour");
    let line =
        format!("\\mode:={mode};mode_setup;message\"BDPI= \"&decimal round pixels_per_inch;end.");
    let r = font_run(io, base, &line, s);
    // (`awk '/DPI=/ {print $2}'`)
    let mf_bdpi = r
        .term
        .split(|&b| b == b'\n')
        .filter(|l| l.windows(4).any(|w| w == b"DPI="))
        .filter_map(|l| {
            l.split(|b| b.is_ascii_whitespace())
                .filter(|w| !w.is_empty())
                .nth(1)
                .map(<[u8]>::to_vec)
        })
        .next_back();
    if mf_bdpi.as_deref() == Some(bdpi.to_string().as_bytes()) && mode != "default" {
        return Ok(mode.to_string());
    }
    guess_mode(bdpi)
        .map(str::to_string)
        .ok_or_else(|| format!("mktexpk: Can't guess mode for {bdpi} dpi devices."))
}

/// A font METAFONT made: the PK at the resolution asked for, and the TFM.
pub struct Made {
    /// `NAME.DPIpk`.
    pub pk_name: String,
    pub pk: Vec<u8>,
    pub gf: Vec<u8>,
    pub tfm: Option<Vec<u8>>,
}

/// `mktexpk --dpi DPI --bdpi BDPI --mag MAKETEX_MAG --mfmode MODE NAME`:
/// METAFONT on `NAME` from `base`, the GF packed by gftopk.
///
/// # Errors
/// What went wrong, METAFONT's terminal output where it made no GF file.
pub fn mktexpk(
    io: Box<dyn Io>,
    base: &[u8],
    name: &str,
    dpi: i32,
    bdpi: i32,
    s: &Settings,
) -> Result<Made, String> {
    let mag = maketex_mag(dpi, bdpi);
    let line = format!("\\mode:={}; mag:={mag}; nonstopmode; input {name}", s.mode);
    let r = font_run(io, base, &line, s);
    let gf_name = format!("{name}.{dpi}gf");
    let gf = r
        .output(&gf_name)
        .ok_or_else(|| r.failure(&gf_name))?
        .to_vec();
    let pk = phitex_gftopk::gf_to_pk(&gf)?;
    Ok(Made {
        pk_name: format!("{name}.{dpi}pk"),
        pk,
        gf,
        tfm: r.output(&format!("{name}.tfm")).map(<[u8]>::to_vec),
    })
}

/// `mktextfm NAME`: METAFONT on `NAME` at `mag:=1` in mode `MODE`.
///
/// # Errors
/// METAFONT's terminal output where it made no TFM file.
pub fn mktextfm(io: Box<dyn Io>, base: &[u8], name: &str, s: &Settings) -> Result<Vec<u8>, String> {
    let line = format!("\\mode:={}; mag:=1; ; nonstopmode; input {name}", s.mode);
    let r = font_run(io, base, &line, s);
    let t = format!("{name}.tfm");
    r.output(&t)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| r.failure(&t))
}

/// METAFONT as `mf-nowin -progname=mf LINE` with `mf.base`.
#[must_use]
pub fn font_run(io: Box<dyn Io>, base: &[u8], line: &str, s: &Settings) -> Run {
    let cfg = Config {
        ini: false,
        dump_name: "mf".into(),
        command_line: vec![line.as_bytes().to_vec()],
        date: s.date,
        version_string: s.version_string.clone(),
        ..Config::default()
    };
    run(cfg, io, Some((base, "mf")))
}

// ---- kpathsea's miss: a font made where its file is asked for ----

/// A font file kpathsea makes where its search finds none (`kpse_make_tex`
/// for `kpse_tfm_format` and `kpse_pk_format`): what a finder that missed
/// is asked for, by the name it was asked ([`Request::of`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `mktextfm NAME`: `NAME.tfm`.
    Tfm { font: String },
    /// `mktexpk --dpi DPI --bdpi BDPI --mfmode MODE NAME`: `NAME.DPIpk`,
    /// asked as `NAME.DPIpk@BDPI[:MODE]` (the glyph settings pdfTeX gives
    /// `kpse_init_prog`; `phitex_pdf::pk_request`). A plain `NAME.DPIpk`
    /// has none: kpathsea makes none.
    Pk {
        font: String,
        dpi: u32,
        bdpi: u32,
        mode: Option<String>,
    },
}

impl Request {
    /// The request a finder's miss of `name` is, if kpathsea would make
    /// that file (`cmr10.tfm`; `cmr10.600pk@600:ljfour`).
    #[must_use]
    pub fn of(name: &str) -> Option<Request> {
        if let Some(font) = name.strip_suffix(".tfm") {
            return (!font.is_empty() && !font.contains('/')).then(|| Request::Tfm {
                font: font.to_owned(),
            });
        }
        let (file, settings) = name.split_once('@')?;
        let (font, ext) = file.rsplit_once('.')?;
        let dpi = ext.strip_suffix("pk")?;
        if font.is_empty() || font.contains('/') || dpi.is_empty() {
            return None;
        }
        let (bdpi, mode) = settings
            .split_once(':')
            .map_or((settings, None), |(b, m)| (b, Some(m.to_owned())));
        Some(Request::Pk {
            font: font.to_owned(),
            dpi: dpi.parse().ok()?,
            bdpi: bdpi.parse().ok()?,
            mode,
        })
    }

    /// The font's name.
    #[must_use]
    pub fn font(&self) -> &str {
        match self {
            Request::Tfm { font } | Request::Pk { font, .. } => font,
        }
    }

    /// The METAFONT source the request needs (mktex's `kpsewhich
    /// NAME.mf`): none found, no font made.
    #[must_use]
    pub fn source(&self) -> String {
        format!("{}.mf", self.font())
    }

    /// The file made: `NAME.tfm`, `NAME.DPIpk`.
    #[must_use]
    pub fn output(&self) -> String {
        match self {
            Request::Tfm { font } => format!("{font}.tfm"),
            Request::Pk { font, dpi, .. } => format!("{font}.{dpi}pk"),
        }
    }

    /// The run's key in a [`Cache`] (`mktexpk cmr10 600 600 ljfour`).
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Request::Tfm { font } => format!("mktextfm {font}"),
            Request::Pk {
                font,
                dpi,
                bdpi,
                mode,
            } => format!(
                "mktexpk {font} {dpi} {bdpi} {}",
                mode.as_deref().unwrap_or("/")
            ),
        }
    }
}

/// The search a file of a run is found by: METAFONT's inputs
/// (`MFINPUTS`, `mf.ini` and the fonts' sources) or web2c's files
/// (`cp227.tcx`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Mf,
    Web2c,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Mf => "mf",
            Kind::Web2c => "web2c",
        }
    }

    fn of_tag(t: &str) -> Option<Kind> {
        match t {
            "mf" => Some(Kind::Mf),
            "web2c" => Some(Kind::Web2c),
            _ => None,
        }
    }
}

/// Where a run's files come from, as `mf-nowin -progname=mf` finds them
/// (kpathsea on a machine, Shelf, a job's own written files first): the
/// path found and the bytes.
pub trait Files {
    fn find(&mut self, name: &[u8], kind: Kind) -> Option<(Vec<u8>, Vec<u8>)>;

    /// Whether a lookup was put off (a file not had yet: the engine looks
    /// it up and runs again, `phitex_tex`'s tool runs): a kept run is not
    /// made anew while its files are not all had.
    fn pending(&self) -> bool {
        false
    }
}

/// What a run made, kept: the files it looked up (their search, name and
/// content hash, none where not found) and its outputs by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub reads: Vec<(Kind, Vec<u8>, Option<u64>)>,
    pub outputs: Vec<(String, Vec<u8>)>,
}

impl Kept {
    /// The manifest of the reads, a line each: `search<TAB>hash<TAB>name`
    /// (`-` for a file not found).
    #[must_use]
    pub fn manifest(&self) -> String {
        let mut m = String::new();
        for (k, name, h) in &self.reads {
            let h = h.map_or_else(|| "-".to_owned(), |h| format!("{h:016x}"));
            m.push_str(&format!(
                "{}\t{h}\t{}\n",
                k.tag(),
                String::from_utf8_lossy(name)
            ));
        }
        m
    }

    /// The reads of a [`Kept::manifest`].
    #[must_use]
    pub fn reads_of(manifest: &str) -> Option<Vec<(Kind, Vec<u8>, Option<u64>)>> {
        manifest
            .lines()
            .map(|l| {
                let mut w = l.splitn(3, '\t');
                let k = Kind::of_tag(w.next()?)?;
                let h = w.next()?;
                let h = if h == "-" {
                    None
                } else {
                    Some(u64::from_str_radix(h, 16).ok()?)
                };
                Some((k, w.next()?.as_bytes().to_vec(), h))
            })
            .collect()
    }
}

/// Where runs' results are kept between builds (the CLI's cache
/// directory; a process's memory, [`MemoryCache`]). A result is given
/// again only while every file its run looked up still has its content.
pub trait Cache: Send + Sync {
    fn get(&self, key: &str) -> Option<Kept>;
    fn put(&self, key: &str, kept: &Kept);
}

/// A [`Cache`] in the process's memory.
#[derive(Default)]
pub struct MemoryCache(std::sync::Mutex<std::collections::HashMap<String, Kept>>);

impl Cache for MemoryCache {
    fn get(&self, key: &str) -> Option<Kept> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }
    fn put(&self, key: &str, kept: &Kept) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_owned(), kept.clone());
    }
}

/// A content hash of a file a run read (FNV-1a, 64 bits).
#[must_use]
pub fn content_hash(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325, |h, &c| {
        (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A run's lookups through [`Files`], each kept for the [`Kept`]
/// manifest: METAFONT's [`Io`] ([`Rec::io`]) and the run's own reads.
#[derive(Clone)]
struct Rec<F> {
    files: F,
    reads: std::rc::Rc<std::cell::RefCell<Vec<(Kind, Vec<u8>, Option<u64>)>>>,
}

impl<F: Files + Clone + 'static> Rec<F> {
    /// `name` as `files` finds it: its path and bytes.
    fn find(&mut self, name: &[u8], kind: Kind) -> Option<(Vec<u8>, Vec<u8>)> {
        let found = self.files.find(name, kind);
        self.reads.borrow_mut().push((
            kind,
            name.to_vec(),
            found.as_ref().map(|(_, b)| content_hash(b)),
        ));
        found
    }

    /// METAFONT's files through these lookups.
    fn io(&self) -> Box<dyn Io> {
        Box::new(self.clone())
    }
}

impl<F: Files + Clone + 'static> Io for Rec<F> {
    fn open_in(&mut self, name: &[u8], kind: FileKind) -> Option<(Vec<u8>, Vec<u8>)> {
        (kind == FileKind::Mf)
            .then(|| self.find(name, Kind::Mf))
            .flatten()
            .map(|(p, b)| (b, p))
    }
}

/// The outputs of the run keyed `key`: from `cache` where every file it
/// looked up is as it read it (looked up again through `files`), else
/// made by `make` and kept.
fn cached<F: Files + Clone + 'static>(
    key: &str,
    files: &F,
    cache: &dyn Cache,
    make: impl FnOnce(&mut Rec<F>) -> Result<Vec<(String, Vec<u8>)>, String>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    if let Some(k) = cache.get(key) {
        let mut f = files.clone();
        if k.reads
            .iter()
            .all(|(kind, name, h)| f.find(name, *kind).map(|(_, b)| content_hash(&b)) == *h)
        {
            return Ok(k.outputs);
        }
        if f.pending() {
            return Err(format!("{key}: its files not had yet"));
        }
    }
    let mut rec = Rec {
        files: files.clone(),
        reads: std::rc::Rc::default(),
    };
    let outputs = make(&mut rec)?;
    let kept = Kept {
        reads: std::mem::take(&mut *rec.reads.borrow_mut()),
        outputs,
    };
    // (a run that missed a file not had yet is not the run: not kept)
    if files.pending() {
        return Err(format!("{key}: its files not had yet"));
    }
    cache.put(key, &kept);
    Ok(kept.outputs)
}

/// The output named `name` of a run's outputs.
fn output_of(out: Vec<(String, Vec<u8>)>, name: &str) -> Result<Vec<u8>, String> {
    out.into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, b)| b)
        .ok_or_else(|| format!("{name}: not made"))
}

/// `mf.base` as fmtutil builds it (`mf.ini` with `cp227.tcx`), through
/// `files`, kept in `cache`.
///
/// # Errors
/// What went wrong: no TCX file, METAFONT's terminal output.
pub fn base<F: Files + Clone + 'static>(
    files: &F,
    cache: &dyn Cache,
    s: &Settings,
) -> Result<Vec<u8>, String> {
    let out = cached("mf.base", files, cache, |rec| {
        let (_, tcx) = rec
            .find(b"cp227.tcx", Kind::Web2c)
            .ok_or("mf: cp227.tcx not found")?;
        let b = build_base(rec.io(), Some(("cp227.tcx", &tcx)), s)?;
        Ok(vec![("mf.base".into(), b)])
    })?;
    output_of(out, "mf.base")
}

/// `mktexmf NAME`: the source TeX Live's script writes for a font whose
/// own `NAME.mf` is not found, from its root (`NAME` without its last
/// digits, the point size): a Sauter font's `b-ROOT.mf`, else by the
/// name's family (EC and TC, DC, CS, LH, CB Greek), else `ROOT.mf` at the
/// size (`cmti5`: `design_size := 5; input cmti;`). None where the root
/// has no source (the script's `exit 1`).
pub fn mktexmf<F: Files>(name: &str, files: &mut F) -> Option<Vec<u8>> {
    let root = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let size = &name[root.len()..];
    let has = |files: &mut F, n: &str| files.find(n.as_bytes(), Kind::Mf).is_some();
    let sauter = format!("b-{root}");
    let (root, found) = if has(files, &format!("{sauter}.mf")) {
        (sauter.as_str(), true)
    } else {
        let b = name.as_bytes();
        let at = |i: usize, set: &[u8]| b.get(i).is_some_and(|c| set.contains(c));
        let source = if name.starts_with("csso12")
            && b.len() == 8
            && at(6, b"012345")
            && b[7].is_ascii_digit()
        {
            None
        } else if ["cs", "lcsss", "icscsc", "icstt", "ilcsss"]
            .iter()
            .any(|p| root.starts_with(p))
        {
            Some("cscode.mf".to_owned())
        } else if lh_name(root) {
            Some(format!("{}codes.mf", &name[..2]))
        } else {
            Some(format!("{root}.mf"))
        };
        (root, source.is_some_and(|s| has(files, &s)))
    };
    if !found {
        return None;
    }
    let real = match size {
        "" => return None,
        "11" => "10.95".to_owned(),
        "14" => "14.4".to_owned(),
        "17" => "17.28".to_owned(),
        "20" => "20.74".to_owned(),
        "25" => "24.88".to_owned(),
        "30" => "29.86".to_owned(),
        "36" => "35.83".to_owned(),
        s if s.len() == 4 || s.len() == 5 => format!("{}.{}", &s[..s.len() - 2], &s[s.len() - 2..]),
        s => s.to_owned(),
    };
    let text = if name.starts_with("ec") || name.starts_with("tc") {
        format!("if unknown exbase: input exbase fi;\ngensize:={real};\ngenerate {root};\n")
    } else if name.starts_with("dc") {
        format!("if unknown dxbase: input dxbase fi;\ngensize:={real};\ngenerate {root};\n")
    } else if ["cs", "lcsss", "icscsc", "icstt", "ilcsss"]
        .iter()
        .any(|p| name.starts_with(p))
    {
        "input cscode\nuse_driver;\n".to_owned()
    } else if lh_name(name) {
        "input fikparm;\n".to_owned()
    } else if cbgreek_name(name) {
        "% generated by mktexmf\ninput cbgreek;\n".to_owned()
    } else {
        format!("design_size := {real};\ninput {root};\n")
    };
    Some(text.into_bytes())
}

/// mktexmf's LH names: `wn[bcdfirstuv]*`, `rx[bcdfiorstuvx][bcfhilmostx]*`,
/// `l[abcdhl][bcdfiorstuvx]*`.
fn lh_name(n: &str) -> bool {
    let b = n.as_bytes();
    let at = |i: usize, set: &[u8]| b.get(i).is_some_and(|c| set.contains(c));
    (n.starts_with("wn") && at(2, b"bcdfirstuv"))
        || (n.starts_with("rx") && at(2, b"bcdfiorstuvx") && at(3, b"bcfhilmostx"))
        || (n.starts_with('l') && at(1, b"abcdhl") && at(2, b"bcdfiorstuvx"))
}

/// mktexmf's CB Greek names: `g[lmorst][bijmtwx][cilnoru]*`.
fn cbgreek_name(n: &str) -> bool {
    let b = n.as_bytes();
    let at = |i: usize, set: &[u8]| b.get(i).is_some_and(|c| set.contains(c));
    n.starts_with('g') && at(1, b"lmorst") && at(2, b"bijmtwx") && at(3, b"cilnoru")
}

/// [`Files`] with a source mktexmf wrote ([`mktexmf`]) found first.
#[derive(Clone)]
struct WithSource<F> {
    files: F,
    name: Vec<u8>,
    text: std::sync::Arc<[u8]>,
}

impl<F: Files> Files for WithSource<F> {
    fn find(&mut self, name: &[u8], kind: Kind) -> Option<(Vec<u8>, Vec<u8>)> {
        // (as METAFONT asks for it, `input cmti5`, or by its file name)
        let asked = name == &self.name[..]
            || self
                .name
                .strip_suffix(b".mf")
                .is_some_and(|stem| name == stem);
        if kind == Kind::Mf && asked {
            return Some((name.to_vec(), self.text.to_vec()));
        }
        self.files.find(name, kind)
    }

    fn pending(&self) -> bool {
        self.files.pending()
    }
}

/// The file kpathsea's miss `req` makes (`mktextfm`, `mktexpk`), as TeX
/// Live's scripts make it: none where the font has no METAFONT source
/// (`kpsewhich NAME.mf` finds none and [`mktexmf`] writes none; not
/// here: the PostScript fonts gsftopk renders); else METAFONT on the
/// source with the base ([`base`]), every file through `files`, the runs
/// kept in `cache`.
///
/// # Errors
/// What went wrong where the source is found: METAFONT's terminal output
/// where it made no font, no mode for the resolution.
pub fn make<F: Files + Clone + 'static>(
    req: &Request,
    files: &F,
    cache: &dyn Cache,
    s: &Settings,
) -> Result<Option<Vec<u8>>, String> {
    let source = req.source();
    if files.clone().find(source.as_bytes(), Kind::Mf).is_some() {
        return make_from(req, files, cache, s).map(Some);
    }
    let Some(text) = mktexmf(req.font(), &mut files.clone()) else {
        return Ok(None);
    };
    let with = WithSource {
        files: files.clone(),
        name: source.into_bytes(),
        text: text.into(),
    };
    make_from(req, &with, cache, s).map(Some)
}

/// [`make`] where the font's source is found.
fn make_from<F: Files + Clone + 'static>(
    req: &Request,
    files: &F,
    cache: &dyn Cache,
    s: &Settings,
) -> Result<Vec<u8>, String> {
    let base = base(files, cache, s)?;
    let name = req.output();
    // (the base the run is made with in its key: a font kept is one
    // made from this base)
    let key = format!("{} base {:016x}", req.key(), content_hash(&base));
    let out = cached(&key, files, cache, |rec| match req {
        Request::Tfm { font } => {
            let b = mktextfm(rec.io(), &base, font, s)?;
            Ok(vec![(name.clone(), b)])
        }
        Request::Pk {
            font,
            dpi,
            bdpi,
            mode,
        } => {
            let (dpi, bdpi) = (
                i32::try_from(*dpi).map_err(|e| e.to_string())?,
                i32::try_from(*bdpi).map_err(|e| e.to_string())?,
            );
            let mode = mktexpk_mode(rec.io(), &base, mode.as_deref(), bdpi, s)?;
            let s = Settings { mode, ..s.clone() };
            let made = mktexpk(rec.io(), &base, font, dpi, bdpi, &s)?;
            Ok(vec![(made.pk_name, made.pk)])
        }
    })?;
    output_of(out, &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files of these names only, empty.
    #[derive(Clone)]
    struct Names(&'static [&'static str]);

    impl Files for Names {
        fn find(&mut self, name: &[u8], _: Kind) -> Option<(Vec<u8>, Vec<u8>)> {
            self.0
                .iter()
                .any(|n| n.as_bytes() == name)
                .then(|| (name.to_vec(), Vec::new()))
        }
    }

    #[test]
    fn requests() {
        assert_eq!(
            Request::of("cmr10.tfm"),
            Some(Request::Tfm {
                font: "cmr10".into()
            })
        );
        assert_eq!(
            Request::of("dancers.600pk@600:ljfour"),
            Some(Request::Pk {
                font: "dancers".into(),
                dpi: 600,
                bdpi: 600,
                mode: Some("ljfour".into())
            })
        );
        // (a PK file asked with no glyph settings: kpathsea makes none)
        assert_eq!(Request::of("cmr10.600pk"), None);
        assert_eq!(Request::of("article.cls"), None);
    }

    /// mktexmf's sources (TeX Live's script): a Sauter font's root first,
    /// else the root at the size, the magsteps' sizes and four digits'
    /// hundredths; none without a root.
    #[test]
    fn mktexmf_sources() {
        let text = |n: &str, f: &'static [&'static str]| {
            mktexmf(n, &mut Names(f)).map(|b| String::from_utf8(b).unwrap())
        };
        assert_eq!(
            text("cmti5", &["b-cmti.mf", "cmti.mf"]).as_deref(),
            Some("design_size := 5;\ninput b-cmti;\n")
        );
        assert_eq!(
            text("cmr14", &["cmr.mf"]).as_deref(),
            Some("design_size := 14.4;\ninput cmr;\n")
        );
        assert_eq!(
            text("ecrm1095", &["ecrm.mf"]).as_deref(),
            Some("if unknown exbase: input exbase fi;\ngensize:=10.95;\ngenerate ecrm;\n")
        );
        assert_eq!(text("cmtt5", &[]), None);
        assert_eq!(text("cmr", &["cmr.mf"]), None);
    }

    #[test]
    fn maketex_mags() {
        assert_eq!(maketex_mag(600, 600), "1+0/600");
        // (kpathsea's, at magsteps too: TeX Live's mktexpk runs, mc job 77344)
        assert_eq!(maketex_mag(864, 600), "1+264/600");
        assert_eq!(maketex_mag(264, 600), "0+264/600");
        assert_eq!(maketex_mag(720, 600), "1+120/600");
        assert_eq!(maketex_mag(1000, 600), "1+400/600");
        assert_eq!(maketex_mag(60, 600), "0+60/600");
    }
}
