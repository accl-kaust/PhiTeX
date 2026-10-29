//! XeTeX's native fonts for a build (docs/spec-xetex.md §3): font files
//! through kpathsea as `xetex` finds them (`[file]` names, TECkit
//! mappings), and the font index a name (`"Latin Modern Roman/B"`) is
//! looked up in: TeX Live's OpenType and TrueType fonts in the order
//! fontconfig lists them, as XeTeX's `FcFontList` sees them (the faces
//! `crates/fonts/otf/examples/otf-index.rs` indexes for the oracle).
//! The index is made once per font list, kept in the cache by the list's
//! hash and by the font directories' (their modification times, as
//! fontconfig's cache checks them): while they are the same, a process
//! reads the index without running `fc-list`. Without `fc-list` there is
//! none, and only `[file]` names load.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_otf::index::FaceEntry;
use phitex_otf::xetex::fontmgr::fc_names;
use phitex_tex::{FontIndex, FontSource, KpseFormat, XeTeX};

/// Font files by kpathsea as `xetex`.
struct KpseFonts(Mutex<phitex_kpse::Kpse>);

impl FontSource for KpseFonts {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        std::fs::read(path).ok().map(Arc::from)
    }

    fn find_file(&self, name: &str, format: KpseFormat) -> Option<String> {
        let fmt = match format {
            KpseFormat::OpenType => phitex_kpse::Format::OpenType,
            KpseFormat::TrueType => phitex_kpse::Format::TrueType,
            KpseFormat::Type1 => phitex_kpse::Format::Type1,
            KpseFormat::MiscFonts => phitex_kpse::Format::MiscFonts,
        };
        let mut k = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let p = k.find_file(name.as_bytes(), fmt, false)?;
        String::from_utf8(p).ok()
    }
}

/// The builds' XeTeX: made once a process.
pub fn xetex() -> XeTeX {
    static X: OnceLock<XeTeX> = OnceLock::new();
    X.get_or_init(|| {
        let mut k = crate::tex::kpse("xetex", "xetex");
        let index = font_index(&mut k).map(Arc::new);
        XeTeX {
            fonts: Some(Arc::new(KpseFonts(Mutex::new(k)))),
            font_index: index,
            pdf: None,
        }
    })
    .clone()
}

/// One face as fontconfig lists it.
struct FcFace {
    file: String,
    index: u32,
    weight: u16,
    width: u16,
    slant: u16,
    fullname_empty: bool,
    style_empty: bool,
}

/// fontconfig's list of TeX Live's `fonts/opentype` and `fonts/truetype`
/// with XeTeX's `FcFontList` objects (their order is the name
/// matching's), as text.
fn fc_list(conf: &Path) -> Option<String> {
    let sep = '\u{1f}';
    let fmt = format!(
        "%{{file}}\t%{{index}}\t%{{weight}}\t%{{width}}\t%{{slant}}\t%{{[]family{{%{{family}}{sep}}}}}\t%{{[]style{{%{{style}}{sep}}}}}\t%{{[]fullname{{%{{fullname}}{sep}}}}}\n"
    );
    // (fontconfig takes `SOURCE_DATE_EPOCH` for its caches' time: set,
    // it finds them stale and reads every font again, a second)
    let out = Command::new("fc-list")
        .env("FONTCONFIG_FILE", conf)
        .env_remove("SOURCE_DATE_EPOCH")
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

fn faces(text: &str) -> Vec<FcFace> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 8 {
                return None;
            }
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "fontconfig's small numbers"
            )]
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

/// The index of the fonts fontconfig lists, from the cache when the
/// list is the one it was made of.
fn font_index(k: &mut phitex_kpse::Kpse) -> Option<FontIndex> {
    let dist = PathBuf::from(String::from_utf8(k.var_value("TEXMFDIST")?).ok()?);
    let cache = crate::tex::cache_dir();
    std::fs::create_dir_all(&cache).ok()?;
    let conf = cache.join("xetex-fonts.conf");
    let text = format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n<fontconfig>\n  <dir>{}</dir>\n  <dir>{}</dir>\n  <cachedir>{}</cachedir>\n</fontconfig>\n",
        dist.join("fonts/opentype").display(),
        dist.join("fonts/truetype").display(),
        cache.join("fontconfig").display()
    );
    if std::fs::read_to_string(&conf).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&conf, &text).ok()?;
    }
    // (the font directories as they are, with fontconfig's own: the
    // index made of the list they gave, without listing them again)
    let dirs = dirs_key(&[dist.join("fonts/opentype"), dist.join("fonts/truetype")]);
    let by_dirs = cache.join(format!("xetex-fonts-dirs-{dirs:016x}.idx"));
    if let Some(ix) = std::fs::read(&by_dirs)
        .ok()
        .and_then(|b| FontIndex::from_bytes(&b))
    {
        return Some(ix);
    }
    let list = fc_list(&conf)?;
    let key = crate::tex::Fnv::new().add(list.as_bytes()).0;
    let path = cache.join(format!("xetex-fonts-{key:016x}.idx"));
    if let Some(b) = std::fs::read(&path).ok()
        && let Some(ix) = FontIndex::from_bytes(&b)
    {
        put(&cache, &by_dirs, &b);
        return Some(ix);
    }
    let mut index = FontIndex::default();
    for f in faces(&list) {
        let Ok(data) = std::fs::read(&f.file) else {
            continue;
        };
        let mut e = FaceEntry::read(&f.file, Arc::from(data), f.index);
        // (fontconfig's pattern for a variable font itself)
        if f.index == 0 && f.style_empty && f.fullname_empty && !e.names.is_empty() {
            e.flags |= phitex_otf::index::VARIABLE;
        }
        // (XeTeX reads fontconfig's weight, width and slant where the
        // face has no OS/2 weight and width)
        if e.os2.is_none_or(|(w, wd, _)| w == 0 && wd == 0) {
            e.fc_style = Some((f.weight, f.width, f.slant));
        }
        e.fc_names = Some(fc_names(&e));
        index.entries.push(e);
    }
    let b = index.to_bytes();
    put(&cache, &path, &b);
    put(&cache, &by_dirs, &b);
    Some(index)
}

/// `bytes` written at `path` in the cache, through a temporary file
/// renamed.
fn put(cache: &Path, path: &Path, bytes: &[u8]) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = cache.join(format!(".{name}.tmp"));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// A key of the font directories under `roots` as they are: each
/// directory's path and modification time (a font added, removed or
/// renamed changes its directory's, as fontconfig's cache sees it), and
/// `fc-list`'s.
fn dirs_key(roots: &[PathBuf]) -> u64 {
    let mut h = crate::tex::Fnv::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    let mut dirs = Vec::new();
    while let Some(d) = stack.pop() {
        let Ok(m) = std::fs::metadata(&d) else {
            continue;
        };
        let t = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
        dirs.push((d.clone(), t.map_or(0, |t| t.as_nanos())));
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(e.path());
                }
            }
        }
    }
    dirs.sort();
    for (d, t) in &dirs {
        h = h
            .add(d.as_os_str().as_encoded_bytes())
            .add(&t.to_le_bytes());
    }
    let fc = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("fc-list"))
            .find(|f| f.is_file())
    });
    if let Some(f) = fc {
        let t = std::fs::metadata(&f)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |t| t.as_nanos());
        h = h
            .add(f.as_os_str().as_encoded_bytes())
            .add(&t.to_le_bytes());
    }
    h.0
}
