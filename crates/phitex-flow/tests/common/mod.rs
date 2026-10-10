//! Test support: TeX Live's kpathsea for the builds the tests make. The
//! finder is a given of the build: the library never looks for TeX Live,
//! the tests do, here (no process is run).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use phitex_flow::Build;
use phitex_flow::kpse::Kpathsea;

/// The directory of TeX Live's programs: `SELFAUTOLOC` from the
/// environment, else that of `kpsewhich` (or `tex`) on `PATH`, links
/// resolved.
pub fn bin_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("SELFAUTOLOC").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let path = std::env::var_os("PATH")?;
    ["kpsewhich", "tex"].iter().find_map(|p| {
        std::env::split_paths(&path)
            .filter_map(|d| d.join(p).canonicalize().ok())
            .find(|f| f.is_file())
            .and_then(|f| f.parent().map(Path::to_path_buf))
    })
}

/// kpathsea as program `prog` (engine `engine`), if a TeX Live is found.
pub fn kpathsea_as(prog: &str, engine: &str) -> Option<Kpathsea> {
    let mut k = partex_kpse::Kpse::new(&bin_dir()?, prog, engine);
    k.var_value("TEXMF")?;
    Some(Kpathsea::new(k))
}

/// TeX Live's kpathsea, one per test binary (it switches programs as
/// asked); none without a TeX Live or with `PHITEX_NO_KPSE` set.
pub fn kpathsea() -> Option<Arc<Kpathsea>> {
    static K: OnceLock<Option<Arc<Kpathsea>>> = OnceLock::new();
    K.get_or_init(|| {
        if std::env::var_os("PHITEX_NO_KPSE").is_some_and(|v| !v.is_empty()) {
            return None;
        }
        kpathsea_as("pdftex", "pdftex").map(Arc::new)
    })
    .clone()
}

/// `b` finds files by kpathsea as program `prog` where a TeX Live is found
/// (else as before: its directories, TeX Live's walked).
pub fn give_kpse(b: &mut Build, prog: &str) {
    if let Some(k) = kpathsea() {
        b.set_kpse(k, prog);
    }
}
