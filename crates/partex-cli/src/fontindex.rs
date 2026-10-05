//! `XeTeX`'s installed fonts for the native host (DESIGN 4.7): the index
//! its font names are looked up in (`partex_otf::index`), built from what
//! fontconfig lists, as `XeTeX` asks it (`FcFontList` with `:outline=true`
//! and its objects, in fontconfig's order, which name matching depends
//! on). The listing names the index in the cache: the faces are read once
//! for a given set.

use std::process::Command;
use std::sync::{Arc, OnceLock};

use partex_otf::index::{FaceEntry, FontIndex, VARIABLE};

/// A face as `fc-list` gives it.
struct FcFace {
    file: String,
    index: u32,
    weight: u16,
    width: u16,
    slant: u16,
    style_empty: bool,
    fullname_empty: bool,
}

/// `fc-list`'s output with `XeTeX`'s objects (`FONTCONFIG_FILE` applies).
fn fc_list() -> Option<String> {
    let sep = '\u{1f}';
    let fmt = format!(
        "%{{file}}\t%{{index}}\t%{{weight}}\t%{{width}}\t%{{slant}}\t%{{[]family{{%{{family}}{sep}}}}}\t%{{[]style{{%{{style}}{sep}}}}}\t%{{[]fullname{{%{{fullname}}{sep}}}}}\n"
    );
    let out = Command::new("fc-list")
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
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn faces(listing: &str) -> Vec<FcFace> {
    listing
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 8 {
                return None;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let num = |s: &str| s.parse::<f64>().map_or(0, |v| v as u16);
            let empty = |s: &str| s.split('\u{1f}').all(str::is_empty);
            Some(FcFace {
                file: f[0].to_string(),
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

/// The index of the installed fonts (made once a process; `None` without
/// fontconfig).
pub fn index() -> Option<Arc<[u8]>> {
    static INDEX: OnceLock<Option<Arc<[u8]>>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            let listing = fc_list()?;
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (b"partex-fontindex-1", &listing).hash(&mut h);
                u128::from(h.finish())
            };
            if let Some(b) = crate::cache::get(key)
                && FontIndex::from_bytes(&b).is_some()
            {
                return Some(Arc::from(b));
            }
            let mut index = FontIndex::default();
            for f in faces(&listing) {
                let Ok(data) = std::fs::read(&f.file) else {
                    continue;
                };
                let mut e = FaceEntry::read(&f.file, Arc::from(data), f.index);
                // fontconfig's pattern for a variable font itself
                if f.index == 0 && f.style_empty && f.fullname_empty && !e.names.is_empty() {
                    e.flags |= VARIABLE;
                }
                // (`XeTeX` reads fontconfig's weight, width and slant where
                // the face gives no OS/2 weight and width)
                if e.os2.is_none_or(|(w, wd, _)| w == 0 && wd == 0) {
                    e.fc_style = Some((f.weight, f.width, f.slant));
                }
                index.entries.push(e);
            }
            let bytes = index.to_bytes();
            crate::cache::put(key, &bytes);
            Some(Arc::from(bytes))
        })
        .clone()
}
