//! The cache on disk: `formats/` (a format's evaluated state, shared by
//! every document; named by its engine and its ini text's hash, and
//! stored with `phitex-tex`'s prelude key, which the engine checks against
//! the files the prelude read) and `docs/ID/`
//! (one document's, ID a hash of its main file's path). Entries are
//! written whole (a temporary file renamed), so processes can share the
//! cache. A size limit holds it: after each write the entries used least
//! recently (their files' times, set on each use) go until it fits, the
//! ones a build is using kept. `phitex clean` empties the document's part,
//! `phitex clean --all` all of it.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::tex;

/// The limit when none is set: 4 GB.
pub const DEFAULT_LIMIT: u64 = 4_000_000_000;

/// The cache, at its directory, with its limit (0: no cache).
pub struct Cache {
    pub dir: PathBuf,
    pub limit: u64,
}

/// One file of the cache: its path, size and last use.
struct Entry {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

impl Cache {
    /// The cache at [`tex::cache_dir`], its limit `PHITEX_CACHE_LIMIT`,
    /// else `configured`, else [`DEFAULT_LIMIT`].
    pub fn open(configured: Option<u64>) -> Result<Cache, String> {
        let env = match std::env::var("PHITEX_CACHE_LIMIT") {
            Ok(v) if !v.trim().is_empty() => Some(
                crate::config::parse_size(&v)
                    .ok_or_else(|| format!("PHITEX_CACHE_LIMIT={v}: not a size"))?,
            ),
            _ => None,
        };
        Ok(Cache {
            dir: tex::cache_dir(),
            limit: env.or(configured).unwrap_or(DEFAULT_LIMIT),
        })
    }

    /// Why formats would not be kept from run to run (none: they are):
    /// the directory can't be written, or it is the temporary one (no
    /// `$HOME`), so the format is built again, unseen, every time.
    pub fn unkept(&self) -> Option<String> {
        let formats = self.dir.join("formats");
        let probe = formats.join(format!(".probe-{}", std::process::id()));
        let wrote = std::fs::create_dir_all(&formats).and_then(|()| std::fs::write(&probe, b""));
        let _ = std::fs::remove_file(&probe);
        if let Err(e) = wrote {
            return Some(format!(
                "can't write the cache at {} ({e}): the format is built again every run (PHITEX_CACHE_DIR sets the cache)",
                self.dir.display()
            ));
        }
        self.dir.starts_with(std::env::temp_dir()).then(|| {
            format!(
                "the cache is in {} (no $HOME): it may not last (PHITEX_CACHE_DIR sets it)",
                self.dir.display()
            )
        })
    }

    /// The directory of the document whose main file is `main`.
    pub fn doc_dir(&self, main: &Path) -> PathBuf {
        let p = main.canonicalize().unwrap_or_else(|_| main.to_path_buf());
        let id = tex::Fnv::new().add(p.to_string_lossy().as_bytes()).0;
        self.dir.join("docs").join(format!("{id:016x}"))
    }

    fn format_path(&self, name: &str) -> PathBuf {
        self.dir.join("formats").join(name)
    }

    /// A format's evaluated state stored as `name`, with the key the
    /// engine gave it (its use noted).
    pub fn format(&self, name: &str) -> Option<([u8; 32], Vec<u8>)> {
        let b = self.get(&self.format_path(name))?;
        let key = b.get(..32)?.try_into().ok()?;
        Some((key, b[32..].to_vec()))
    }

    /// A format's evaluated state stored as `name` with its key, the
    /// cache held to its limit.
    pub fn put_format(&self, name: &str, key: &[u8; 32], bytes: &[u8]) -> io::Result<()> {
        let p = self.format_path(name);
        let mut b = key.to_vec();
        b.extend_from_slice(bytes);
        self.put(&p, &b, std::slice::from_ref(&p))
    }

    /// The bytes at `path` in the cache, its use noted (none: no cache,
    /// or not there).
    pub fn get(&self, path: &Path) -> Option<Vec<u8>> {
        if self.limit == 0 {
            return None;
        }
        let b = std::fs::read(path).ok()?;
        if let Ok(f) = std::fs::File::options().write(true).open(path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(b)
    }

    /// `bytes` stored at `path` (in the cache's directory), whole; then
    /// the least recently used entries evicted to the limit, `keep` kept.
    pub fn put(&self, path: &Path, bytes: &[u8], keep: &[PathBuf]) -> io::Result<()> {
        if self.limit == 0 {
            return Ok(());
        }
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        self.evict(keep);
        Ok(())
    }

    fn entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        let mut todo = vec![self.dir.clone()];
        while let Some(d) = todo.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let Ok(m) = e.metadata() else { continue };
                if m.is_dir() {
                    todo.push(e.path());
                } else {
                    out.push(Entry {
                        path: e.path(),
                        size: m.len(),
                        used: m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    });
                }
            }
        }
        out
    }

    /// The bytes the cache holds.
    pub fn usage(&self) -> u64 {
        self.entries().iter().map(|e| e.size).sum()
    }

    /// The least recently used entries removed until the cache fits its
    /// limit (`keep` never). The bytes removed.
    pub fn evict(&self, keep: &[PathBuf]) -> u64 {
        let mut es = self.entries();
        let mut total: u64 = es.iter().map(|e| e.size).sum();
        es.sort_by_key(|e| e.used);
        let mut freed = 0;
        for e in es {
            if total <= self.limit {
                break;
            }
            if keep.contains(&e.path) {
                continue;
            }
            if std::fs::remove_file(&e.path).is_ok() {
                total -= e.size;
                freed += e.size;
            }
        }
        freed
    }

    /// Removes directory `d` of the cache; the bytes it held.
    fn remove(&self, d: &Path) -> io::Result<u64> {
        let held = Cache {
            dir: d.to_path_buf(),
            limit: self.limit,
        }
        .usage();
        match std::fs::remove_dir_all(d) {
            Ok(()) => Ok(held),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// The document's part emptied; the bytes freed.
    pub fn clear_doc(&self, main: &Path) -> io::Result<u64> {
        self.remove(&self.doc_dir(main))
    }

    /// All of it emptied; the bytes freed.
    pub fn clear_all(&self) -> io::Result<u64> {
        self.remove(&self.dir)
    }

    /// The line after a build or a watch: what the cache holds, and how
    /// to empty it.
    pub fn summary(&self) -> String {
        let dir = home_relative(&self.dir);
        if self.limit == 0 {
            return format!("cache: off (cache-limit = 0) · {dir}");
        }
        format!(
            "cache: {} of {} in {dir} (phitex clean)",
            size(self.usage()),
            size(self.limit)
        )
    }
}

/// `p` with the home directory as `~`.
pub fn home_relative(p: &Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(h) if !h.as_os_str().is_empty() && p.starts_with(&h) => {
            format!("~/{}", p.strip_prefix(&h).unwrap_or(p).display())
        }
        _ => p.display().to_string(),
    }
}

/// Bytes for people: `0 B`, `12.3 MB`, `4.0 GB`.
pub fn size(b: u64) -> String {
    bytesize::ByteSize::b(b).display().si().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("phitex-cache-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn evicts_least_recently_used() {
        let c = Cache {
            dir: scratch("lru"),
            limit: 25,
        };
        let p = |n: &str| c.dir.join("formats").join(n);
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        for (k, n) in ["a", "b"].iter().enumerate() {
            c.put(&p(n), &[0; 10], &[]).unwrap();
            let f = std::fs::File::options().write(true).open(p(n)).unwrap();
            f.set_modified(old + std::time::Duration::from_secs(k as u64))
                .unwrap();
        }
        // (a used now: b is the least recently used)
        assert!(c.get(&p("a")).is_some());
        c.put(&p("c"), &[0; 10], &[p("c")]).unwrap();
        assert!(p("a").is_file() && p("c").is_file());
        assert!(!p("b").exists(), "the least recently used entry stayed");
        assert_eq!(c.usage(), 20);
        assert_eq!(c.clear_all().unwrap(), 20);
        assert_eq!(c.usage(), 0);
    }

    #[test]
    fn keeps_what_is_in_use_and_off_is_off() {
        let c = Cache {
            dir: scratch("keep"),
            limit: 5,
        };
        let big = c.dir.join("formats").join("big");
        c.put(&big, &[0; 10], std::slice::from_ref(&big)).unwrap();
        assert!(big.is_file(), "the entry in use was evicted");
        let doc = c.doc_dir(Path::new("/x/paper.tex"));
        let state = doc.join("state");
        c.put(&state, &[1], &[big.clone(), state.clone()]).unwrap();
        assert_eq!(c.clear_doc(Path::new("/x/paper.tex")).unwrap(), 1);
        assert!(big.is_file());
        let _ = c.clear_all();
        let off = Cache {
            dir: scratch("off"),
            limit: 0,
        };
        let f = off.dir.join("f");
        off.put(&f, &[1], &[]).unwrap();
        assert!(!f.exists() && off.get(&f).is_none());
    }

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(12_345_678), "12.3 MB");
        assert_eq!(size(DEFAULT_LIMIT), "4.0 GB");
    }
}
