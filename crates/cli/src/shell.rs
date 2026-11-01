//! The native runner of `\write18`'s commands (`phitex_tex::ShellHook`,
//! docs/note-shell-hook.md): `/bin/sh -c` in a fresh scratch directory
//! per call holding only the files the engine hands it, with kpathsea's
//! variables exported (latexminted's `latexrestricted` finds TeX Live by
//! `SELFAUTOLOC`) and the document's directory first on `TEXINPUTS`, so
//! `kpsewhich` finds the project's files (read only, untracked).
//! (Self-contained: the engine's tests run TeX Live's commands with it,
//! `crates/engines/tex/tests/latex.rs`.)

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use phitex_tex::{ShellHook, ShellRan};

/// Runs a command as a process (`/bin/sh -c`).
pub struct ProcessHook {
    /// The variables the command's environment gets.
    env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl ProcessHook {
    /// The runner for a document in directory `doc_dir`, the commands'
    /// environment with kpathsea's variables `exported` (its `xputenv`s:
    /// `SELFAUTOLOC` and the others, `progname`, `engine`).
    pub fn new<'a>(
        doc_dir: &Path,
        exported: impl Iterator<Item = (&'a [u8], &'a [u8])>,
    ) -> ProcessHook {
        use std::os::unix::ffi::OsStrExt;
        let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> = exported
            .map(|(a, b)| {
                (
                    std::ffi::OsStr::from_bytes(a).to_owned(),
                    std::ffi::OsStr::from_bytes(b).to_owned(),
                )
            })
            .collect();
        env.sort();
        // (the project's files found by kpsewhich, the document's
        // directory first, then the default path: the trailing colon)
        let dir = std::path::absolute(doc_dir).unwrap_or_else(|_| doc_dir.to_path_buf());
        let mut inputs = dir.into_os_string();
        inputs.push(":");
        if let Some(t) = std::env::var_os("TEXINPUTS") {
            inputs.push(t);
        }
        env.push(("TEXINPUTS".into(), inputs));
        ProcessHook { env }
    }
}

/// A scratch directory of its own per call.
static CALLS: AtomicU64 = AtomicU64::new(0);

/// Whether reading a file moves its access time here (not `noatime`):
/// probed once, on a file of the process's own, read as this process
/// reads and as a command reads it (`cat` copying it: coreutils copies
/// with `copy_file_range`, which leaves the access time alone on some
/// file systems, overlayfs among them, so a command's reads there are
/// not seen and the files it read would be taken for unread).
fn atime_moves() -> bool {
    static MOVES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *MOVES.get_or_init(|| {
        let d = scratch();
        let ok = (|| -> std::io::Result<bool> {
            std::fs::create_dir_all(&d)?;
            let p = d.join("probe");
            std::fs::write(&p, b"probe")?;
            unread(&p)?;
            let _ = std::fs::read(&p)?;
            if !was_read(&p) {
                return Ok(false);
            }
            unread(&p)?;
            let copied = std::process::Command::new("/bin/sh")
                .args(["-c", "cat probe > copy"])
                .current_dir(&d)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            Ok(copied.success() && was_read(&p))
        })()
        .unwrap_or(false);
        let _ = std::fs::remove_dir_all(&d);
        ok
    })
}

fn scratch() -> PathBuf {
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("phitex-shell-{}-{n}", std::process::id()))
}

/// The access time of `p` set to the epoch: a read moves it (relatime
/// moves an access time older than a day).
fn unread(p: &Path) -> std::io::Result<()> {
    let f = std::fs::File::open(p)?;
    f.set_times(std::fs::FileTimes::new().set_accessed(SystemTime::UNIX_EPOCH))
}

fn was_read(p: &Path) -> bool {
    std::fs::metadata(p)
        .and_then(|m| m.accessed())
        .is_ok_and(|t| t != SystemTime::UNIX_EPOCH)
}

/// The files under `root` by their `/`-separated paths relative to it,
/// and the directories (hidden ones skipped; at most 100 000 entries).
fn tree(root: &Path) -> (Vec<String>, Vec<PathBuf>) {
    let (mut files, mut dirs) = (Vec::new(), vec![root.to_path_buf()]);
    let mut todo = vec![(root.to_path_buf(), String::new())];
    let mut seen = 0usize;
    while let Some((d, rel)) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > 100_000 {
                return (files, dirs);
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                if !name.starts_with('.') {
                    dirs.push(e.path());
                    todo.push((e.path(), r));
                }
            } else if t.is_file() {
                files.push(r);
            }
        }
    }
    files.sort();
    (files, dirs)
}

/// A name the engine hands that stays inside the scratch directory.
fn inside(name: &str) -> bool {
    let p = Path::new(name);
    !name.is_empty()
        && !p.is_absolute()
        && p.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}

impl ShellHook for ProcessHook {
    fn run(&self, command: &str, files: &[(String, Arc<[u8]>)]) -> ShellRan {
        use std::os::unix::process::ExitStatusExt;
        let track = atime_moves();
        let dir = scratch();
        let fail = |status| ShellRan {
            status,
            ..ShellRan::default()
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return fail(127 << 8);
        }
        let given: Vec<&(String, Arc<[u8]>)> = files.iter().filter(|(n, _)| inside(n)).collect();
        for (n, b) in &given {
            let p = dir.join(n);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&p, &b[..]);
        }
        // (every access time at the epoch, files and directories, after
        // all are written)
        let (_, dirs) = tree(&dir);
        let mut track = track;
        for p in given
            .iter()
            .map(|(n, _)| dir.join(n))
            .chain(dirs.iter().cloned())
        {
            track &= unread(&p).is_ok();
        }
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        // (`system`'s status: the shell's own 127 when it cannot start)
        let status = cmd.status().map_or(127 << 8, ExitStatusExt::into_raw);
        // (what it read, before this reads anything back: a directory
        // listed, all of it)
        let read = if track && !dirs.iter().any(|d| d.is_dir() && was_read(d)) {
            Some(
                given
                    .iter()
                    .filter(|(n, _)| was_read(&dir.join(n)))
                    .map(|(n, _)| n.clone())
                    .collect(),
            )
        } else {
            None
        };
        let (after, _) = tree(&dir);
        // (a given file it removed or changed counts as read: what it did
        // may hang on what it was)
        let read = read.map(|mut r: Vec<String>| {
            for (n, b) in &given {
                let gone = !dir.join(n).is_file();
                let changed = !gone && std::fs::read(dir.join(n)).is_ok_and(|x| x[..] != b[..]);
                if (gone || changed) && !r.contains(n) {
                    r.push(n.clone());
                }
            }
            r
        });
        let mut wrote = Vec::new();
        for n in &after {
            let Ok(b) = std::fs::read(dir.join(n)) else {
                continue;
            };
            if given
                .iter()
                .any(|(m, g)| m.strip_prefix("./").unwrap_or(m) == n && g[..] == b[..])
            {
                continue;
            }
            wrote.push((n.clone(), b));
        }
        let removed = given
            .iter()
            .map(|(n, _)| n.strip_prefix("./").unwrap_or(n).to_owned())
            .filter(|n| !after.contains(n))
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        ShellRan {
            status,
            wrote,
            removed,
            read,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command over the files it is handed: what it wrote, changed and
    /// removed, and what it read.
    #[test]
    fn runs_over_the_files_given() {
        let h = ProcessHook::new(Path::new("."), std::iter::empty());
        let files: Vec<(String, Arc<[u8]>)> = vec![
            ("a.txt".into(), Arc::from(&b"alpha\n"[..])),
            ("b.txt".into(), Arc::from(&b"beta\n"[..])),
            ("sub/c.txt".into(), Arc::from(&b"gamma\n"[..])),
        ];
        let r = h.run(
            "cat a.txt > out/../made.txt; rm sub/c.txt; echo x >> b.txt",
            &files,
        );
        // (`out/` does not exist: the shell fails that redirection)
        assert_eq!(r.removed, vec!["sub/c.txt".to_owned()]);
        assert!(
            r.wrote
                .iter()
                .any(|(n, b)| n == "b.txt" && b == b"beta\nx\n")
        );
        let r = h.run("cat a.txt > made.txt", &files);
        assert_eq!(r.status, 0);
        assert_eq!(r.wrote, vec![("made.txt".to_owned(), b"alpha\n".to_vec())]);
        assert_eq!(r.removed, Vec::<String>::new());
        if let Some(read) = &r.read {
            assert_eq!(read, &vec!["a.txt".to_owned()]);
        }
        assert_eq!(h.run("exit 3", &files).status, 3 << 8);
    }
}
