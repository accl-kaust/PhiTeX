//! XeTeX's native fonts for a build (docs/spec-xetex.md §3.1): font
//! files through kpathsea as `xetex` finds them (`[file]` names in the
//! `opentype fonts`, `truetype fonts` and `type1 fonts` formats, the
//! current directory and `OSFONTDIR` among their paths; TECkit mappings),
//! and the font index a name (`"Latin Modern Roman/B"`) is looked up in:
//! the fonts fontconfig lists, in its order, as XeTeX's `FcFontList`
//! sees them — the system's, the user's (`~/.local/share/fonts`,
//! `~/.fonts`), whatever `fonts.conf` names (`FONTCONFIG_FILE` read as
//! fontconfig reads it) — and after them TeX Live's `fonts/opentype` and
//! `fonts/truetype` faces fontconfig does not list (as on a system whose
//! fontconfig has TeX Live's `texlive-fontconfig.conf`, the install TeX
//! Live asks for). The faces are those
//! `crates/fonts/otf/examples/otf-index.rs` indexes for the oracle.
//!
//! The index is made once per font list and kept in the cache by the
//! list's hash, and by a key of fontconfig's configuration files and font
//! directories (their paths and modification times, as fontconfig's
//! cache checks them, [`fc_key`]): while they are the same, a process
//! reads the index without running `fc-list`; a font added to a font
//! directory changes the key. Without `fc-list` there is none, and only
//! `[file]` names load.

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
            ..XeTeX::default()
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

/// fontconfig's list of fonts, as XeTeX's `FcFontList` sees it
/// (`XeTeXFontMgr_FC::initialize`: the default configuration,
/// `FONTCONFIG_FILE` and the rest of the environment as they are, the
/// pattern `:outline=true`, its objects; their order is the name
/// matching's), as text.
fn fc_list(conf: Option<&Path>) -> Option<String> {
    let sep = '\u{1f}';
    let fmt = format!(
        "%{{file}}\t%{{index}}\t%{{weight}}\t%{{width}}\t%{{slant}}\t%{{[]family{{%{{family}}{sep}}}}}\t%{{[]style{{%{{style}}{sep}}}}}\t%{{[]fullname{{%{{fullname}}{sep}}}}}\n"
    );
    // (fontconfig takes `SOURCE_DATE_EPOCH` for its caches' time: set,
    // it finds them stale and reads every font again, a second; the list
    // is the same)
    let mut c = Command::new("fc-list");
    if let Some(conf) = conf {
        c.env("FONTCONFIG_FILE", conf);
    }
    let out = c
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

/// The index of the fonts fontconfig lists, and of TeX Live's
/// `fonts/opentype` and `fonts/truetype` where its list has them not
/// (docs/spec-xetex.md §3.1: TeX Live's fonts by name, as on a system
/// whose fontconfig has TeX Live's `texlive-fontconfig.conf`); from the
/// cache while fontconfig's configuration and the font directories are
/// as they were when it was made ([`fc_key`]), else from the lists.
fn font_index(k: &mut phitex_kpse::Kpse) -> Option<FontIndex> {
    let cache = crate::tex::cache_dir();
    std::fs::create_dir_all(&cache).ok()?;
    let tl = k
        .var_value("TEXMFDIST")
        .and_then(|d| String::from_utf8(d).ok())
        .map(|d| {
            [
                PathBuf::from(&d).join("fonts/opentype"),
                PathBuf::from(&d).join("fonts/truetype"),
            ]
        });
    let conf = fcconf::Config::read();
    let extra_at = cache.join(format!("xetex-fonts-extra-{:016x}.txt", conf.key()));
    let mut extra: Vec<PathBuf> = std::fs::read_to_string(&extra_at)
        .map(|t| t.lines().map(PathBuf::from).collect())
        .unwrap_or_default();
    let watched: Vec<PathBuf> = extra.iter().chain(tl.iter().flatten()).cloned().collect();
    let by_dirs = cache.join(format!(
        "xetex-fonts-fc-{:016x}.idx",
        fc_key(&conf, &watched)
    ));
    if let Some(ix) = std::fs::read(&by_dirs)
        .ok()
        .and_then(|b| FontIndex::from_bytes(&b))
    {
        return Some(ix);
    }
    let mut list = fc_list(None)?;
    // (a font fontconfig lists from a directory the configuration's
    // reading did not name: that directory watched too from now on)
    let mut more: Vec<PathBuf> = faces(&list)
        .iter()
        .filter_map(|f| Path::new(&f.file).parent().map(Path::to_path_buf))
        .filter(|d| {
            !conf
                .font_dirs
                .iter()
                .chain(&watched)
                .any(|r| d.starts_with(r))
        })
        .collect();
    more.sort();
    more.dedup();
    if !more.is_empty() {
        extra.extend(more);
        let text = extra.iter().fold(String::new(), |mut t, d| {
            t.push_str(&d.display().to_string());
            t.push('\n');
            t
        });
        put(&cache, &extra_at, text.as_bytes());
    }
    // (TeX Live's trees as fontconfig lists them by themselves, their
    // faces not in the list already after it)
    if let Some([ot, tt]) = &tl {
        let tl_conf = cache.join("xetex-fonts.conf");
        let text = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n<fontconfig>\n  <dir>{}</dir>\n  <dir>{}</dir>\n  <cachedir>{}</cachedir>\n</fontconfig>\n",
            ot.display(),
            tt.display(),
            cache.join("fontconfig").display()
        );
        if std::fs::read_to_string(&tl_conf).ok().as_deref() != Some(text.as_str()) {
            put(&cache, &tl_conf, text.as_bytes());
        }
        if let Some(tl_list) = fc_list(Some(&tl_conf)) {
            // (a face by its file's real path: fontconfig may list TeX
            // Live's through a link, `/usr/share/fonts/texlive`)
            let real = |f: &str| std::fs::canonicalize(f).unwrap_or_else(|_| PathBuf::from(f));
            let have: std::collections::HashSet<(PathBuf, u32)> = faces(&list)
                .into_iter()
                .map(|f| (real(&f.file), f.index))
                .collect();
            for line in tl_list.lines() {
                let mut f = line.splitn(3, '\t');
                let (Some(file), Some(index)) = (f.next(), f.next()) else {
                    continue;
                };
                if !have.contains(&(real(file), index.parse().unwrap_or(u32::MAX))) {
                    list.push_str(line);
                    list.push('\n');
                }
            }
        }
    }
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

/// A key of fontconfig's font list as its own cache would see it: the
/// configuration's files and the directories that hold them, each font
/// directory under the configuration's (and `extra`'s) with its
/// subdirectories, by path and modification time (a font added,
/// removed or renamed changes its directory's, as fontconfig's cache
/// checks it; a file or directory not there yet counts as absent), the
/// environment fontconfig reads, and `fc-list`'s file.
fn fc_key(conf: &fcconf::Config, extra: &[PathBuf]) -> u64 {
    let mut h = crate::tex::Fnv::new().add(&conf.key().to_le_bytes());
    let mtime = |p: &Path| -> Option<(std::fs::Metadata, u128)> {
        let m = std::fs::metadata(p).ok()?;
        let t = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |t| t.as_nanos());
        Some((m, t))
    };
    let mut seen: Vec<(PathBuf, u128)> = Vec::new();
    for f in conf.files.iter().chain(&conf.conf_dirs) {
        seen.push((f.clone(), mtime(f).map_or(u128::MAX, |(_, t)| t)));
    }
    let mut stack: Vec<PathBuf> = conf.font_dirs.iter().chain(extra).cloned().collect();
    let mut walked = std::collections::HashSet::new();
    while let Some(d) = stack.pop() {
        if !walked.insert(std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone())) {
            continue;
        }
        let Some((m, t)) = mtime(&d) else {
            seen.push((d, u128::MAX));
            continue;
        };
        seen.push((d.clone(), t));
        if m.is_dir()
            && let Ok(rd) = std::fs::read_dir(&d)
        {
            for e in rd.flatten() {
                // (fontconfig follows links to directories, as `stat`)
                if std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()) {
                    stack.push(e.path());
                }
            }
        }
    }
    seen.sort();
    for (d, t) in &seen {
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
        let t = mtime(&f).map_or(0, |(_, t)| t);
        h = h
            .add(f.as_os_str().as_encoded_bytes())
            .add(&t.to_le_bytes());
    }
    h.0
}

/// fontconfig's configuration as far as its font list depends on it:
/// the files it reads and the font directories they name, found as
/// fontconfig finds them (`FcConfigFilename`, `<include>`, `<dir>`).
/// Read for [`fc_key`] alone: a directory or file named here that
/// fontconfig does not read only makes the key change more often, never
/// the list.
mod fcconf {
    use std::path::{Path, PathBuf};

    /// The environment fontconfig's configuration reads.
    const ENV: [&str; 6] = [
        "FONTCONFIG_FILE",
        "FONTCONFIG_PATH",
        "FONTCONFIG_SYSROOT",
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
    ];

    /// fontconfig's own configuration directory (its `FONTCONFIG_PATH`
    /// as built: `/etc/fonts` on Linux distributions, `/usr/local/etc/fonts`
    /// for a build of its own).
    const DEFAULT_DIRS: [&str; 2] = ["/etc/fonts", "/usr/local/etc/fonts"];

    #[derive(Default)]
    pub struct Config {
        /// The configuration files read (or looked for: `ignore_missing`).
        pub files: Vec<PathBuf>,
        /// The directories included whole (a `conf.d`): a file added there
        /// changes the directory's time.
        pub conf_dirs: Vec<PathBuf>,
        /// The font directories (`<dir>`), as named.
        pub font_dirs: Vec<PathBuf>,
    }

    fn var(name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    }

    fn home() -> Option<PathBuf> {
        var("HOME").map(PathBuf::from)
    }

    /// `~/x` as fontconfig expands it.
    fn tilde(s: &str) -> Option<PathBuf> {
        match s.strip_prefix('~') {
            Some(r) => Some(home()?.join(r.trim_start_matches('/'))),
            None => Some(PathBuf::from(s)),
        }
    }

    /// `$XDG_CONFIG_HOME` or `~/.config`; `$XDG_DATA_HOME` or `~/.local/share`.
    fn xdg(config: bool) -> Option<PathBuf> {
        let (v, d) = if config {
            ("XDG_CONFIG_HOME", ".config")
        } else {
            ("XDG_DATA_HOME", ".local/share")
        };
        var(v).map(PathBuf::from).or_else(|| Some(home()?.join(d)))
    }

    /// The text's XML entities fontconfig's parser (expat) decodes.
    fn unescape(s: &str) -> String {
        s.trim()
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    }

    /// An attribute's value in a start tag's text.
    fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let mut rest = tag;
        while let Some(i) = rest.find(name) {
            let before = rest[..i].chars().last();
            let after = rest[i + name.len()..].trim_start();
            rest = &rest[i + name.len()..];
            if !before.is_some_and(char::is_whitespace) {
                continue;
            }
            let Some(v) = after.strip_prefix('=') else {
                continue;
            };
            let v = v.trim_start();
            let q = v.chars().next()?;
            if q != '"' && q != '\'' {
                continue;
            }
            let v = &v[1..];
            return Some(&v[..v.find(q)?]);
        }
        None
    }

    impl Config {
        /// The configuration of this process's environment.
        pub fn read() -> Config {
            let mut c = Config::default();
            let path: Vec<PathBuf> = var("FONTCONFIG_PATH")
                .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .chain(DEFAULT_DIRS.iter().map(PathBuf::from))
                .collect();
            let name = var("FONTCONFIG_FILE").unwrap_or_else(|| "fonts.conf".to_owned());
            // (`FcConfigFilename`: `~/…`, an absolute name, else the first
            // directory of the configuration path that has it)
            let main = if name.starts_with('~') || name.starts_with('/') {
                tilde(&name)
            } else {
                path.iter()
                    .map(|d| d.join(&name))
                    .find(|p| p.is_file())
                    .or_else(|| path.first().map(|d| d.join(&name)))
            };
            let sysroot = var("FONTCONFIG_SYSROOT").map(PathBuf::from);
            if let Some(main) = main {
                let main = match &sysroot {
                    Some(r) if main.is_absolute() && !main.starts_with(r) => {
                        r.join(main.strip_prefix("/").unwrap_or(&main))
                    }
                    _ => main,
                };
                c.file(&main, 0);
            }
            if let Some(r) = &sysroot {
                c.font_dirs = c
                    .font_dirs
                    .iter()
                    .map(|d| {
                        if d.is_absolute() && !d.starts_with(r) {
                            r.join(d.strip_prefix("/").unwrap_or(d))
                        } else {
                            d.clone()
                        }
                    })
                    .collect();
            }
            c
        }

        /// The key of what was read: the files, the directories, the
        /// environment.
        pub fn key(&self) -> u64 {
            let mut h = crate::tex::Fnv::new();
            for v in ENV {
                h = h
                    .add(v.as_bytes())
                    .add(&[0])
                    .add(var(v).unwrap_or_default().as_bytes())
                    .add(&[0]);
            }
            for (k, list) in [
                (b'f', &self.files),
                (b'c', &self.conf_dirs),
                (b'd', &self.font_dirs),
            ] {
                for p in list {
                    h = h.add(&[k]).add(p.as_os_str().as_encoded_bytes()).add(&[0]);
                }
            }
            h.0
        }

        /// One configuration file (or a directory of them) read, at
        /// include depth `depth`.
        pub(super) fn file(&mut self, p: &Path, depth: usize) {
            if depth > 16 || self.files.iter().any(|f| f == p) {
                return;
            }
            if p.is_dir() {
                self.conf_dirs.push(p.to_path_buf());
                // (`FcConfigParseAndLoadDir`: the `*.conf` files in name order)
                let mut names: Vec<PathBuf> = std::fs::read_dir(p)
                    .map(|rd| rd.flatten().map(|e| e.path()).collect())
                    .unwrap_or_default();
                names.retain(|n| n.extension().is_some_and(|e| e == "conf"));
                names.sort();
                for n in names {
                    self.file(&n, depth + 1);
                }
                return;
            }
            self.files.push(p.to_path_buf());
            let Ok(text) = std::fs::read_to_string(p) else {
                return;
            };
            let here = p.parent().unwrap_or(Path::new("/")).to_path_buf();
            let mut rest = text.as_str();
            while let Some(i) = rest.find('<') {
                rest = &rest[i..];
                if let Some(r) = rest.strip_prefix("<!--") {
                    rest = r.find("-->").map_or("", |e| &r[e + 3..]);
                    continue;
                }
                let Some(close) = rest.find('>') else { break };
                let tag = &rest[1..close];
                rest = &rest[close + 1..];
                let elem = tag.split_whitespace().next().unwrap_or("");
                if (elem != "dir" && elem != "include") || tag.ends_with('/') {
                    continue;
                }
                let Some(end) = rest.find(&format!("</{elem}")) else {
                    break;
                };
                let body = unescape(&rest[..end]);
                rest = &rest[end..];
                if body.is_empty() {
                    continue;
                }
                let prefix = attr(tag, "prefix");
                let target = match prefix {
                    Some("xdg") => xdg(elem == "include").map(|b| b.join(&body)),
                    Some("relative") => Some(here.join(&body)),
                    _ if body.starts_with('~') => tilde(&body),
                    _ if Path::new(&body).is_absolute() => Some(PathBuf::from(&body)),
                    // (a relative `<include>`: the configuration
                    // directory's; a relative `<dir>`: the current
                    // directory's, fontconfig's "default" and "cwd")
                    _ if elem == "include" => Some(here.join(&body)),
                    _ => std::env::current_dir().ok().map(|d| d.join(&body)),
                };
                let Some(target) = target else { continue };
                if elem == "include" {
                    self.file(&target, depth + 1);
                } else if !self.font_dirs.contains(&target) {
                    self.font_dirs.push(target);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fcconf::Config;

    /// fontconfig's configuration read for the key: `<include>`d files
    /// and directories (their `*.conf` in name order), `<dir>`s as
    /// fontconfig expands them, comments skipped.
    #[test]
    fn the_configuration_names_its_files_and_font_dirs() {
        let d = std::env::temp_dir().join(format!("phitex-fcconf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("conf.d")).expect("conf.d");
        let main = d.join("fonts.conf");
        std::fs::write(
            &main,
            "<?xml version=\"1.0\"?>\n<fontconfig>\n<!-- <dir>/not/this</dir> -->\n<dir>/usr/share/fonts</dir>\n<dir prefix=\"relative\">local-fonts</dir>\n<include ignore_missing=\"yes\">conf.d</include>\n<include ignore_missing=\"yes\">/no/such.conf</include>\n</fontconfig>\n",
        )
        .expect("fonts.conf");
        std::fs::write(
            d.join("conf.d/20-b.conf"),
            "<fontconfig><dir>/b &amp; c</dir></fontconfig>",
        )
        .expect("b");
        std::fs::write(
            d.join("conf.d/10-a.conf"),
            "<fontconfig><dir>/a</dir></fontconfig>",
        )
        .expect("a");
        std::fs::write(d.join("conf.d/README"), "<dir>/not/read</dir>").expect("readme");
        let mut c = Config::default();
        c.file(&main, 0);
        let s = |v: &[std::path::PathBuf]| {
            v.iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            s(&c.font_dirs),
            [
                "/usr/share/fonts".to_owned(),
                d.join("local-fonts").display().to_string(),
                "/a".to_owned(),
                "/b & c".to_owned()
            ]
        );
        assert_eq!(
            s(&c.files),
            [
                main.display().to_string(),
                d.join("conf.d/10-a.conf").display().to_string(),
                d.join("conf.d/20-b.conf").display().to_string(),
                "/no/such.conf".to_owned()
            ]
        );
        assert_eq!(s(&c.conf_dirs), [d.join("conf.d").display().to_string()]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
