//! mktexpk and mktextfm as kpathsea runs them on a miss, in process:
//! METAFONT (`phitex_mf`) on the font's `.mf` source found as
//! `mf-nowin -progname=mf` finds it, with the base fmtutil builds
//! (`mf.ini`, `cp227.tcx`). What a run made is kept in the cache
//! directory with the files it read and their hashes, and given again
//! while every one of them still has its content: a font is made once,
//! not once a build.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_kpse::{Format, Kpse};
use phitex_mf::mktex::{self, Settings};
use phitex_mf::{FileKind, Io};

use crate::tex::{cache_dir, fingerprint, kpse, os};

/// kpathsea as `mf-nowin -progname=mf` (`$engine` `metafont`).
fn mf_kpse() -> &'static Mutex<Kpse> {
    static K: OnceLock<Mutex<Kpse>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(kpse("mf", "metafont")))
}

/// Where kpathsea finds `name` in `format` for METAFONT.
fn find(name: &[u8], format: Format) -> Option<Vec<u8>> {
    mf_kpse()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .find_file(name, format, false)
}

/// A file a run looked up: its format (`mf`, `web2c`), its name as
/// asked, and its content's hash (none where it was not found).
type Looked = (&'static str, Vec<u8>, Option<u64>);

fn format_of(tag: &str) -> Option<Format> {
    match tag {
        "mf" => Some(Format::Mf),
        "web2c" => Some(Format::Web2c),
        _ => None,
    }
}

/// METAFONT's files by kpathsea, each lookup kept ([`Looked`]).
#[derive(Clone, Default)]
struct Files(Arc<Mutex<Vec<Looked>>>);

impl Files {
    fn read(&self, tag: &'static str, name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let found = format_of(tag)
            .and_then(|f| find(name, f))
            .and_then(|p| Some((std::fs::read(os(&p)).ok()?, p)));
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push((
            tag,
            name.to_vec(),
            found.as_ref().map(|(b, _)| fingerprint(b)),
        ));
        found
    }
}

impl Io for Files {
    fn open_in(&mut self, name: &[u8], kind: FileKind) -> Option<(Vec<u8>, Vec<u8>)> {
        (kind == FileKind::Mf)
            .then(|| self.read("mf", name))
            .flatten()
    }
}

/// The settings TeX Live's runs have: the date texmfmp.c's
/// `get_date_and_time` gives METAFONT (`SOURCE_DATE_EPOCH` in UTC with
/// `FORCE_SOURCE_DATE=1`, else the local time), in the GF and PK files'
/// comments.
fn settings() -> Settings {
    let forced = std::env::var("FORCE_SOURCE_DATE").is_ok_and(|v| v == "1");
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .filter(|_| forced)
        .and_then(|e| e.trim().parse::<i64>().ok())
        .and_then(|e| jiff::Timestamp::from_second(e).ok());
    let now = epoch.map_or_else(jiff::Zoned::now, |t| t.to_zoned(jiff::tz::TimeZone::UTC));
    Settings {
        date: (
            i32::from(now.hour()) * 60 + i32::from(now.minute()),
            i32::from(now.day()),
            i32::from(now.month()),
            i32::from(now.year()),
        ),
        ..Settings::default()
    }
}

/// What a run keyed `key` made (its `outputs`, by name), from the cache
/// where every file it looked up still has its content, else made by
/// `make` and kept. The directory they are in.
fn cached(
    key: &str,
    outputs: &[&str],
    make: impl FnOnce(&Files) -> Result<Vec<(String, Vec<u8>)>, String>,
) -> Option<PathBuf> {
    let key = format!("{key} phitex {}", env!("CARGO_PKG_VERSION"));
    let dir = cache_dir()
        .join("mf")
        .join(format!("{:016x}", fingerprint(key.as_bytes())));
    let manifest = dir.join("read");
    if let Ok(m) = std::fs::read_to_string(&manifest)
        && outputs.iter().all(|o| dir.join(o).is_file())
        && m.lines().all(|l| {
            let mut w = l.splitn(3, '\t');
            let (Some(tag), Some(hash), Some(name)) = (w.next(), w.next(), w.next()) else {
                return false;
            };
            let now = format_of(tag)
                .and_then(|f| find(name.as_bytes(), f))
                .and_then(|p| std::fs::read(os(&p)).ok())
                .map_or_else(|| "-".to_string(), |b| format!("{:016x}", fingerprint(&b)));
            now == hash
        })
    {
        return Some(dir);
    }
    let files = Files::default();
    let outs = match make(&files) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return None;
        }
    };
    std::fs::create_dir_all(&dir).ok()?;
    let put = |name: &str, b: &[u8]| -> Option<()> {
        let tmp = dir.join(format!(".{name}.{}", std::process::id()));
        std::fs::write(&tmp, b).ok()?;
        std::fs::rename(&tmp, dir.join(name)).ok()
    };
    for (name, b) in &outs {
        put(name, b)?;
    }
    let mut m = String::new();
    for (tag, name, hash) in files
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
    {
        let hash = hash.map_or_else(|| "-".to_string(), |h| format!("{h:016x}"));
        m.push_str(&format!(
            "{tag}\t{hash}\t{}\n",
            String::from_utf8_lossy(name)
        ));
    }
    put("read", m.as_bytes())?;
    Some(dir)
}

/// `mf.base` as fmtutil builds it, once a process.
fn base() -> Option<Arc<Vec<u8>>> {
    static B: OnceLock<Option<Arc<Vec<u8>>>> = OnceLock::new();
    B.get_or_init(|| {
        let dir = cached("mf.base", &["mf.base"], |files| {
            let (tcx, _) = files
                .read("web2c", b"cp227.tcx")
                .ok_or("mf: cp227.tcx not found")?;
            let b = mktex::build_base(
                Box::new(files.clone()),
                Some(("cp227.tcx", &tcx)),
                &settings(),
            )?;
            Ok(vec![("mf.base".into(), b)])
        })?;
        std::fs::read(dir.join("mf.base")).ok().map(Arc::new)
    })
    .clone()
}

/// One run at a time: two pages asking for one font make it once.
static MAKING: Mutex<()> = Mutex::new(());

/// `mktexpk --dpi DPI --bdpi BDPI --mag MAKETEX_MAG --mfmode MODE NAME`
/// for a font with a METAFONT source: its `NAME.DPIpk`.
pub fn mktexpk(font: &str, dpi: u32, bdpi: u32, mode: Option<&str>) -> Option<PathBuf> {
    let _one = MAKING.lock().unwrap_or_else(PoisonError::into_inner);
    // (`kpsewhich $NAME.mf`; not here: mktexmf's sources and the
    // PostScript fonts gsftopk renders)
    find(format!("{font}.mf").as_bytes(), Format::Mf)?;
    let base = base()?;
    let pk = format!("{font}.{dpi}pk");
    let key = format!("mktexpk {font} {dpi} {bdpi} {}", mode.unwrap_or("/"));
    let dir = cached(&key, &[&pk], |files| {
        let (dpi, bdpi) = (
            i32::try_from(dpi).map_err(|e| e.to_string())?,
            i32::try_from(bdpi).map_err(|e| e.to_string())?,
        );
        let mode = mktex::mktexpk_mode(Box::new(files.clone()), &base, mode, bdpi, &settings())?;
        let s = Settings { mode, ..settings() };
        let font_made = mktex::mktexpk(Box::new(files.clone()), &base, font, dpi, bdpi, &s)?;
        Ok(vec![(font_made.pk_name, font_made.pk)])
    })?;
    Some(dir.join(pk))
}

/// `mktextfm NAME` for a font with a METAFONT source: its `NAME.tfm`.
pub fn mktextfm(font: &str) -> Option<PathBuf> {
    let _one = MAKING.lock().unwrap_or_else(PoisonError::into_inner);
    find(format!("{font}.mf").as_bytes(), Format::Mf)?;
    let base = base()?;
    let tfm = format!("{font}.tfm");
    let dir = cached(&format!("mktextfm {font}"), &[&tfm], |files| {
        let b = mktex::mktextfm(Box::new(files.clone()), &base, font, &settings())?;
        Ok(vec![(tfm.clone(), b)])
    })?;
    Some(dir.join(tfm))
}
