//! XeTeX's fonts for a build (`Build::set_xetex`): the installed fonts as
//! fontconfig lists them, the one thing kpathsea cannot answer (a font by
//! family and style, `\font\x="Latin Modern Roman"`), and the finder for
//! everything else (`[file.otf]` names, the font files' bytes, xdvipdfmx's
//! maps and encodings).
//!
//! The index is `fc-list`'s faces in fontconfig's order (XeTeX's
//! `FcFontList` objects: XeTeX's name matching depends on the order), each
//! face's names read from its file (partex-otf's `FaceEntry`). It is kept
//! in the cache directory under the hash of `fc-list`'s output: a cache of
//! the installed fonts, made again when they change, never of a document.

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use partex_otf::index::{FaceEntry, FontIndex, VARIABLE};
use phitex_flow::native::Fonts;

use crate::tex::{self, Engine, Fnv};

/// The fonts for engine `e`'s builds: made once per process.
pub fn fonts(e: &Engine, note: &mut dyn FnMut(&str)) -> Result<Arc<Fonts>, String> {
    static ONE: Mutex<Option<Arc<Fonts>>> = Mutex::new(None);
    let mut g = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(f) = &*g {
        return Ok(f.clone());
    }
    let index = index(note)?;
    let kpse = finder(e);
    let f = Arc::new(Fonts::new(Arc::new(index), kpse));
    *g = Some(f.clone());
    Ok(f)
}

/// How XeTeX's builds find files by name: kpathsea as program `e.name`.
fn finder(_e: &Engine) -> Arc<dyn phitex_flow::tfm::Kpse> {
    Arc::new(phitex_flow::tfm::Kpsewhich::new())
}

/// One face as `fc-list` gives it.
struct FcFace {
    file: String,
    index: u32,
    weight: u16,
    width: u16,
    slant: u16,
    style_empty: bool,
    fullname_empty: bool,
}

/// `fc-list` with XeTeX's `FcFontList` objects: its output.
fn fc_list() -> Result<String, String> {
    let sep = '\u{1f}';
    let fmt = format!(
        "%{{file}}\t%{{index}}\t%{{weight}}\t%{{width}}\t%{{slant}}\t%{{[]family{{%{{family}}{sep}}}}}\t%{{[]style{{%{{style}}{sep}}}}}\t%{{[]fullname{{%{{fullname}}{sep}}}}}\n"
    );
    let out = Command::new("fc-list")
        .arg("-f")
        .arg(fmt)
        .arg(":outline=true")
        .args(["family", "style", "file", "index", "fullname", "weight", "width", "slant", "fontformat"])
        .output()
        .map_err(|err| format!("fc-list (fontconfig, for XeTeX's fonts by name): {err}"))?;
    if !out.status.success() {
        return Err(format!("fc-list failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn faces(text: &str) -> Vec<FcFace> {
    let sep = '\u{1f}';
    let empty = |s: &str| s.split(sep).all(str::is_empty);
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 8 {
                return None;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let num = |s: &str| s.parse::<f64>().map_or(0, |v| v as u16);
            Some(FcFace {
                file: f[0].to_owned(),
                index: f[1].parse().ok()?,
                weight: num(f[2]),
                width: num(f[3]),
                slant: num(f[4]),
                style_empty: empty(f[6]),
                fullname_empty: empty(f[7]),
            })
        })
        .collect()
}

/// The installed fonts' index: the cache's, else made now.
fn index(note: &mut dyn FnMut(&str)) -> Result<FontIndex, String> {
    let text = fc_list()?;
    let key = Fnv::new().add(text.as_bytes()).0;
    let dir = tex::cache_dir().join("fonts");
    let path = dir.join(format!("fc-{key:016x}.idx"));
    if let Some(i) = std::fs::read(&path).ok().and_then(|b| FontIndex::from_bytes(&b)) {
        return Ok(i);
    }
    let t = std::time::Instant::now();
    let mut index = FontIndex::default();
    for f in faces(&text) {
        let Ok(data) = std::fs::read(&f.file) else { continue };
        let mut e = FaceEntry::read(&f.file, Arc::from(data), f.index);
        // (fontconfig's pattern for a variable font itself)
        if f.index == 0 && f.style_empty && f.fullname_empty && !e.names.is_empty() {
            e.flags |= VARIABLE;
        }
        // (XeTeX reads fontconfig's weight, width and slant where the face
        // gives no OS/2 weight and width)
        if e.os2.is_none_or(|(w, wd, _)| w == 0 && wd == 0) {
            e.fc_style = Some((f.weight, f.width, f.slant));
        }
        index.entries.push(e);
    }
    let bytes = index.to_bytes();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join(format!(".fc-{key:016x}.{}", std::process::id()));
    if std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &path)).is_ok() {
        remove_others(&dir, &path);
    }
    note(&format!(
        "XeTeX's fonts: {} faces as fontconfig lists them, indexed in {:.1} s ({})",
        index.entries.len(),
        t.elapsed().as_secs_f64(),
        path.display()
    ));
    Ok(index)
}

/// The older indexes (of fonts installed then) removed.
fn remove_others(dir: &Path, keep: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("fc-") && n.ends_with(".idx") && p != keep {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fc_faces() {
        let sep = '\u{1f}';
        let t = format!(
            "/f/a.otf\t0\t80\t100\t0\tLatin Modern Roman{sep}\tRegular{sep}\tLMRoman10-Regular{sep}\n/f/v.ttf\t0\t0\t0\t0\tV{sep}\t\t\nshort\n"
        );
        let f = faces(&t);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].file.as_str(), f[0].index, f[0].weight, f[0].width), ("/f/a.otf", 0, 80, 100));
        assert!(!f[0].style_empty && !f[0].fullname_empty);
        assert!(f[1].style_empty && f[1].fullname_empty);
    }
}
