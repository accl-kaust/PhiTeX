//! mktexpk and mktextfm as kpathsea runs them on a miss, in process: the
//! one path the engine takes where its finder misses a TFM or a PK file
//! (`phitex_mf::mktex::make`), with kpathsea as `mf-nowin -progname=mf`
//! finds METAFONT's files ([`KpseFiles`]), for the CLI's DVI and XDV
//! paths (dvipdfmx's fonts); and the cache directory keeping what a run
//! made ([`DiskCache`], the engine's too): a font is made once, not once
//! a build.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_kpse::{Format, Kpse};
use phitex_mf::mktex::{self, Cache, Files, Kept, Kind, Request, Settings};

use crate::tex::{cache_dir, fingerprint, kpse, os};

/// kpathsea as `mf-nowin -progname=mf` (`$engine` `metafont`).
fn mf_kpse() -> &'static Mutex<Kpse> {
    static K: OnceLock<Mutex<Kpse>> = OnceLock::new();
    K.get_or_init(|| Mutex::new(kpse("mf", "metafont")))
}

/// Where kpathsea as `mf-nowin -progname=mf` finds `name` of `kind`
/// (the engine's [`phitex_tex::ToolSearch::Mf`] and `Web2c`).
pub fn find(name: &str, kind: Kind) -> Option<PathBuf> {
    found(name.as_bytes(), kind).map(|p| os(&p).to_path_buf())
}

fn found(name: &[u8], kind: Kind) -> Option<Vec<u8>> {
    let format = match kind {
        Kind::Mf => Format::Mf,
        Kind::Web2c => Format::Web2c,
    };
    mf_kpse()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .find_file(name, format, false)
}

/// METAFONT's files by kpathsea.
#[derive(Clone, Copy, Default)]
pub struct KpseFiles;

impl Files for KpseFiles {
    fn find(&mut self, name: &[u8], kind: Kind) -> Option<(Vec<u8>, Vec<u8>)> {
        let p = found(name, kind)?;
        let b = std::fs::read(os(&p)).ok()?;
        Some((p, b))
    }
}

/// The runs' results in the cache directory (`mf/<key's hash>/`): the
/// outputs, `made` (their names) and `read` (the manifest of the files
/// the run looked up, [`Kept::manifest`]).
pub struct DiskCache;

impl DiskCache {
    fn dir(key: &str) -> PathBuf {
        let key = format!("{key} phitex {}", env!("CARGO_PKG_VERSION"));
        cache_dir()
            .join("mf")
            .join(format!("{:016x}", fingerprint(key.as_bytes())))
    }
}

impl Cache for DiskCache {
    fn get(&self, key: &str) -> Option<Kept> {
        let dir = Self::dir(key);
        let reads = Kept::reads_of(&std::fs::read_to_string(dir.join("read")).ok()?)?;
        let outputs = std::fs::read_to_string(dir.join("made"))
            .ok()?
            .lines()
            .map(|n| Some((n.to_owned(), std::fs::read(dir.join(n)).ok()?)))
            .collect::<Option<_>>()?;
        Some(Kept { reads, outputs })
    }

    fn put(&self, key: &str, kept: &Kept) {
        let dir = Self::dir(key);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        // (each file whole or not at all: written aside, then renamed;
        // the manifest last, so a run cut short is no result)
        let put = |name: &str, b: &[u8]| -> Option<()> {
            let tmp = dir.join(format!(".{name}.{}", std::process::id()));
            std::fs::write(&tmp, b).ok()?;
            std::fs::rename(&tmp, dir.join(name)).ok()
        };
        for (name, b) in &kept.outputs {
            if put(name, b).is_none() {
                return;
            }
        }
        let names: String = kept
            .outputs
            .iter()
            .flat_map(|(n, _)| [n.as_str(), "\n"])
            .collect();
        let _ =
            put("made", names.as_bytes()).and_then(|()| put("read", kept.manifest().as_bytes()));
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

/// The engine's maker of fonts: METAFONT's runs kept in the cache
/// directory, at the date of [`settings`] (`phitex_tex::Options::mktex`).
pub fn maker() -> phitex_tex::Mktex {
    phitex_tex::Mktex {
        cache: Arc::new(DiskCache),
        settings: settings(),
    }
}

/// One run at a time: two pages asking for one font make it once.
static MAKING: Mutex<()> = Mutex::new(());

/// `req` made ([`mktex::make`]), its file in the cache directory: where
/// it is.
fn made(req: &Request) -> Option<PathBuf> {
    let _one = MAKING.lock().unwrap_or_else(PoisonError::into_inner);
    let b = match mktex::make(req, &KpseFiles, &DiskCache, &settings()) {
        Ok(b) => b?,
        Err(e) => {
            eprintln!("{e}");
            return None;
        }
    };
    let p = DiskCache::dir(&format!("file {}", req.key())).join(req.output());
    std::fs::create_dir_all(p.parent()?).ok()?;
    if std::fs::read(&p).ok().as_deref() != Some(&b[..]) {
        let tmp = p.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, &b).ok()?;
        std::fs::rename(&tmp, &p).ok()?;
    }
    Some(p)
}

/// `mktexpk --dpi DPI --bdpi BDPI --mag MAKETEX_MAG --mfmode MODE NAME`
/// for a font with a METAFONT source: its `NAME.DPIpk`.
pub fn mktexpk(font: &str, dpi: u32, bdpi: u32, mode: Option<&str>) -> Option<PathBuf> {
    made(&Request::Pk {
        font: font.to_owned(),
        dpi,
        bdpi,
        mode: mode.map(str::to_owned),
    })
}

/// `mktextfm NAME` for a font with a METAFONT source: its `NAME.tfm`.
pub fn mktextfm(font: &str) -> Option<PathBuf> {
    made(&Request::Tfm {
        font: font.to_owned(),
    })
}
