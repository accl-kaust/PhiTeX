//! The font index (DESIGN 4.7; format: `phitex_otf::index`) made from
//! fontconfig's list of faces: `fc-list` run with a configuration (the
//! repository's `scripts/xetex/fonts.conf`: TeX Live's `fonts/opentype`
//! and `fonts/truetype`, exactly what the oracle's XeTeX sees) and
//! XeTeX's own `FcFontList` arguments (`:outline=true` and the objects
//! family, style, file, index, fullname, weight, width, slant,
//! fontformat), whose order XeTeX's name matching depends on. Each
//! face's bytes are read from its absolute path, which the index keeps.
//! (As `phitex-otf`'s `otf-index` example makes it; that crate reads no
//! file.)

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use phitex_otf::index::{FaceEntry, FontIndex};
use phitex_otf::xetex::fontmgr::fc_names;

/// A face as `fc-list` lists it.
pub struct FcFace {
    pub file: String,
    pub index: u32,
    pub weight: u16,
    pub width: u16,
    pub slant: u16,
    pub style: Vec<String>,
    pub fullname: Vec<String>,
}

/// The faces `fc-list` lists under configuration file `conf`, in its
/// order.
///
/// # Errors
/// `fc-list` could not be run or failed.
pub fn fc_list(conf: &Path) -> Result<Vec<FcFace>, String> {
    let sep = '\u{1f}';
    let fmt = format!(
        "%{{file}}\t%{{index}}\t%{{weight}}\t%{{width}}\t%{{slant}}\t%{{[]family{{%{{family}}{sep}}}}}\t%{{[]style{{%{{style}}{sep}}}}}\t%{{[]fullname{{%{{fullname}}{sep}}}}}\n"
    );
    let out = Command::new("fc-list")
        .env("FONTCONFIG_FILE", conf)
        .arg("-f")
        .arg(fmt)
        .arg(":outline=true")
        .args([
            "family",
            "style",
            "file",
            "index",
            "fullname",
            "weight",
            "width",
            "slant",
            "fontformat",
        ])
        .output()
        .map_err(|e| format!("fc-list: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "fc-list failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let list = |s: &str| -> Vec<String> {
        s.split(sep)
            .filter(|x| !x.is_empty())
            .map(String::from)
            .collect()
    };
    Ok(text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 8 {
                return None;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let num = |s: &str| s.parse::<f64>().map_or(0, |v| v as u16);
            Some(FcFace {
                file: f[0].to_string(),
                index: f[1].parse().ok()?,
                weight: num(f[2]),
                width: num(f[3]),
                slant: num(f[4]),
                style: list(f[6]),
                fullname: list(f[7]),
            })
        })
        .collect())
}

/// Face `f`'s index entry, its bytes read from its path.
///
/// # Errors
/// The face's file could not be read.
pub fn entry(f: &FcFace) -> Result<FaceEntry, String> {
    let data: Arc<[u8]> =
        Arc::from(std::fs::read(&f.file).map_err(|e| format!("{}: {e}", f.file))?);
    let mut e = FaceEntry::read(&f.file, data, f.index);
    // (fontconfig's pattern for a variable font itself)
    if f.index == 0 && f.style.is_empty() && f.fullname.is_empty() && !e.names.is_empty() {
        e.flags |= phitex_otf::index::VARIABLE;
    }
    // XeTeX reads fontconfig's weight, width and slant where the face
    // gives no OS/2 weight and width.
    if e.os2.is_none_or(|(w, wd, _)| w == 0 && wd == 0) {
        e.fc_style = Some((f.weight, f.width, f.slant));
    }
    // (stored: a job's name lookups read them, not derive them)
    e.fc_names = Some(fc_names(&e));
    Ok(e)
}

/// The index of the faces `fc-list` lists under `conf`.
///
/// # Errors
/// As [`fc_list`] and [`entry`].
pub fn from_fontconfig(conf: &Path) -> Result<FontIndex, String> {
    let mut index = FontIndex::default();
    for f in &fc_list(conf)? {
        index.entries.push(entry(f)?);
    }
    Ok(index)
}

/// The repository's configuration (`scripts/xetex/fonts.conf`) with TeX
/// Live's tree at `texmfdist` and the cache under `cache`: the text of
/// the file to give [`from_fontconfig`].
#[must_use]
pub fn tex_live_conf(template: &str, texmfdist: &str, cache: &str) -> String {
    template
        .replace("/usr/share/texmf-dist", texmfdist)
        .replace("/tmp/phitex-xetex-fc-cache", cache)
}
