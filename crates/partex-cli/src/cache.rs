//! The on-disk cache behind `Host::cache_get`/`cache_put`: one file per
//! key under `$PARTEX_CACHE_DIR`, else `$XDG_CACHE_HOME/partex`, else
//! `~/.cache/partex`. `PARTEX_CACHE=0` turns it off.
//!
//! Keys are qualified by this executable (its path, length and time):
//! a value is only read back by the build that wrote it, so the layout of
//! cached values needs no version of its own.
//!
//! The directory is bounded (`PARTEX_CACHE_MAX` bytes, 2 GiB by default):
//! after each write the least recently used files go until it is under
//! the bound. A hit touches its file, so what is used stays; values of
//! other builds are never read again and age out.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The cache directory and this build's identity (`None`: no cache).
fn setup() -> Option<&'static (PathBuf, u128)> {
    static SETUP: OnceLock<Option<(PathBuf, u128)>> = OnceLock::new();
    SETUP
        .get_or_init(|| {
            if std::env::var_os("PARTEX_CACHE").is_some_and(|v| v == "0") {
                return None;
            }
            let dir = std::env::var_os("PARTEX_CACHE_DIR")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("XDG_CACHE_HOME").map(|d| PathBuf::from(d).join("partex"))
                })
                .or_else(|| {
                    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/partex"))
                })?;
            let exe = std::env::current_exe().ok()?;
            let meta = std::fs::metadata(&exe).ok()?;
            let time = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            let build =
                partex_core::persist_hash(&(exe.as_os_str().as_encoded_bytes(), meta.len(), time));
            Some((dir, build))
        })
        .as_ref()
}

/// The file of `key`, if caching is on.
pub fn path(key: u128) -> Option<PathBuf> {
    let (dir, build) = setup()?;
    let name = partex_core::persist_hash(&(*build, key));
    Some(dir.join(format!("{name:032x}")))
}

/// The directory of this build's own formats under the cache (the modern
/// command line makes them there); other builds' formats, never read
/// again, are removed when it is first made.
///
/// `PARTEX_FORMATS=dir` keeps them in `dir` instead (the tests share
/// them between runs that each have a cache of their own).
pub fn formats_dir() -> Option<PathBuf> {
    let (dir, build) = setup()?;
    let dir = &std::env::var_os("PARTEX_FORMATS").map_or_else(|| dir.clone(), PathBuf::from);
    let own = format!("formats-{build:032x}");
    let d = dir.join(&own);
    if !d.is_dir() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.filter_map(Result::ok) {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("formats-") && name != own {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        std::fs::create_dir_all(&d).ok()?;
    }
    Some(d)
}

/// The value kept under `key`, if any (marked as used).
pub fn get(key: u128) -> Option<Vec<u8>> {
    let p = path(key)?;
    let value = std::fs::read(&p).ok()?;
    if let Ok(f) = std::fs::File::options().append(true).open(&p) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
    Some(value)
}

/// The bound on the directory's size.
fn max_bytes() -> u64 {
    std::env::var("PARTEX_CACHE_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2 << 30)
}

/// Remove the least recently used files of `dir` until it holds at most
/// `max` bytes (files being written, `*.tmp*`, are left alone).
fn collect(dir: &std::path::Path, max: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().contains(".tmp"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().ok(), m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= max {
        return;
    }
    files.sort();
    for (_, len, path) in files {
        if total <= max {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// Keep `value` under `key` (written whole, then renamed into place, so a
/// reader never sees part of it). Failures are ignored: it is a cache.
pub fn put(key: u128, value: &[u8]) {
    let Some(p) = path(key) else { return };
    let Some(dir) = p.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, value).is_ok() && std::fs::rename(&tmp, &p).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    collect(dir, max_bytes());
}

#[cfg(test)]
mod tests {
    use super::collect;
    use std::time::{Duration, SystemTime};

    #[test]
    fn least_recently_used_go_first() {
        let dir = std::env::temp_dir().join(format!("partex-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for (name, age) in [("old", 30), ("mid", 20), ("new", 10)] {
            let p = dir.join(name);
            std::fs::write(&p, [0u8; 100]).unwrap();
            let f = std::fs::File::options().append(true).open(&p).unwrap();
            f.set_modified(now - Duration::from_secs(age)).unwrap();
        }
        std::fs::write(dir.join("x.tmp1"), [0u8; 100]).unwrap();
        collect(&dir, 200);
        let mut left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["mid", "new", "x.tmp1"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
