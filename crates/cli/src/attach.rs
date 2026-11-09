//! Editors attached to a running `phitex watch` (docs/spec-editor-protocol.md
//! §8). Every watch, with `--lsp` or not, serves its language server on a
//! TCP listener on 127.0.0.1 at a port the system chooses, behind a random
//! 128-bit token, and says so in a discovery file of its own:
//!
//! - **The file**: `<pid>.json` in the sessions folder ([`dir`]):
//!   `$XDG_RUNTIME_DIR/phitex/` (else `~/.cache/phitex/sessions/`), on
//!   Windows `%LOCALAPPDATA%\phitex\sessions\`. The folder is the user's
//!   alone (made 0700; one another user owns, or others may write, is
//!   refused) and the file 0600: the token, and the viewer's URL with its
//!   own, are in it. `{pid, main, root, viewer, lsp: {host, port, token},
//!   version, started}`; written again when the viewer or the main file
//!   changes, removed when the watch ends (`q`). A file whose watch is not
//!   running is ignored and removed by its readers ([`sessions`]).
//! - **A connection** must open with `initialize` carrying
//!   `initializationOptions.token` ([`first`]): within [`FIRST_WITHIN`]
//!   and [`FIRST_MAX`] bytes, the token compared in constant time; else it
//!   is closed unanswered. At most [`MAX_PENDING`] wait so at once.
//! - `phitex sessions [--json]` lists the running watches.

use std::fmt::Write as _;
use std::io::{BufReader, Read, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The first message's bound in bytes (headers and body).
pub const FIRST_MAX: usize = 64 * 1024;
/// The first message's bound in time from the connection.
pub const FIRST_WITHIN: Duration = Duration::from_secs(3);
/// Connections waiting for their first message at once (more: closed).
pub const MAX_PENDING: usize = 16;
/// A client's write may wait this long (then it is closed).
pub const WRITE_WITHIN: Duration = Duration::from_secs(2);

/// This watch's socket: its discovery file, its `lsp` entry, its start.
struct Published {
    file: PathBuf,
    lsp: Value,
    started: String,
}

static PUBLISHED: OnceLock<Published> = OnceLock::new();
/// (the file written by one thread at a time)
static WRITING: Mutex<()> = Mutex::new(());

/// The sessions folder: made if missing (0700), refused unless it is the
/// user's alone.
pub fn dir() -> Result<PathBuf, String> {
    let d = folder()?;
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    private(&d)?;
    Ok(d)
}

#[cfg(windows)]
fn folder() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|v| PathBuf::from(v).join("phitex").join("sessions"))
        .ok_or_else(|| "no %LOCALAPPDATA%".to_owned())
}

#[cfg(not(windows))]
fn folder() -> Result<PathBuf, String> {
    Ok(std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || crate::tex::cache_dir().join("sessions"),
            |v| PathBuf::from(v).join("phitex"),
        ))
}

/// The user's id, as the system gives this process's files.
#[cfg(unix)]
fn own_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata("/proc/self")
        .ok()
        .or_else(|| std::fs::metadata(std::env::var_os("HOME")?).ok())
        .map(|m| m.uid())
        .or_else(|| {
            // (no /proc, no home: a file made here is ours)
            let p = std::env::temp_dir().join(format!("phitex-uid-{}", std::process::id()));
            let uid = std::fs::File::create(&p)
                .and_then(|f| f.metadata())
                .ok()
                .map(|m| m.uid());
            let _ = std::fs::remove_file(&p);
            uid
        })
}

/// `d` is a folder (not a link) the user owns that no one else may
/// enter or write; its mode made 0700 where it was wider.
#[cfg(unix)]
fn private(d: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let m = std::fs::symlink_metadata(d).map_err(|e| format!("{}: {e}", d.display()))?;
    if !m.is_dir() {
        return Err(format!("{} is not a folder", d.display()));
    }
    if Some(m.uid()) != own_uid() {
        return Err(format!("{} belongs to another user", d.display()));
    }
    if m.mode() & 0o077 != 0 {
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", d.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn private(d: &Path) -> Result<(), String> {
    let m = std::fs::symlink_metadata(d).map_err(|e| format!("{}: {e}", d.display()))?;
    if m.is_dir() {
        Ok(())
    } else {
        Err(format!("{} is not a folder", d.display()))
    }
}

/// A random 128-bit token, in hex: the system's random bytes
/// (`/dev/urandom`; elsewhere std's hasher keys, which the system's
/// random source seeds).
pub fn token() -> String {
    let mut b = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    if read.is_err() {
        use std::hash::{BuildHasher as _, Hasher as _};
        for (i, half) in b.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_usize(i);
            half.copy_from_slice(&h.finish().to_le_bytes());
        }
    }
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Whether `a` and `b` are the same, in a time that does not depend on
/// where they differ.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A connection read under a bound in bytes and time until [`Guarded::free`].
pub struct Guarded {
    s: TcpStream,
    until: Option<Instant>,
    left: usize,
}

impl Guarded {
    fn new(s: TcpStream) -> Guarded {
        Guarded {
            s,
            until: Some(Instant::now() + FIRST_WITHIN),
            left: FIRST_MAX,
        }
    }

    /// No bound from now on.
    fn free(&mut self) {
        self.until = None;
        self.left = usize::MAX;
        let _ = self.s.set_read_timeout(None);
    }
}

impl Read for Guarded {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(until) = self.until {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            self.s.set_read_timeout(Some(left))?;
        }
        if self.left == 0 {
            return Err(std::io::Error::other("the first message is too long"));
        }
        let n = buf.len().min(self.left);
        let got = self.s.read(&mut buf[..n])?;
        self.left -= got;
        Ok(got)
    }
}

/// The connection's first message, if it is `initialize` with `token`
/// (`initializationOptions.token`) and came within the bounds; the rest
/// of the connection unbounded after it.
pub fn first(s: TcpStream, token: &str) -> Option<(Value, BufReader<Guarded>)> {
    let mut r = BufReader::new(Guarded::new(s));
    let m = crate::lsp::read_capped(&mut r, FIRST_MAX)?;
    let given = m["params"]["initializationOptions"]["token"]
        .as_str()
        .unwrap_or("");
    if m["method"] != "initialize" || !same(given.as_bytes(), token.as_bytes()) {
        return None;
    }
    r.get_mut().free();
    Some((m, r))
}

/// Serve the socket: a listener on 127.0.0.1, its token, the discovery
/// file written ([`publish`] with `info`); each connection that passes
/// [`first`] given to `serve` on a thread of its own.
pub fn open<F>(info: &Value, serve: F) -> Result<(), String>
where
    F: Fn(Value, BufReader<Guarded>, TcpStream) + Send + Sync + 'static,
{
    let d = dir()?;
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .map_err(|e| format!("127.0.0.1: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = token();
    let _ = PUBLISHED.set(Published {
        file: d.join(format!("{}.json", std::process::id())),
        lsp: json!({"host": "127.0.0.1", "port": port, "token": token}),
        started: jiff::Timestamp::now().to_string(),
    });
    publish(info);
    let serve = Arc::new(serve);
    let pending = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new()
        .name("phitex-attach".into())
        .spawn(move || {
            for s in listener.incoming().flatten() {
                if pending.fetch_add(1, Ordering::AcqRel) >= MAX_PENDING {
                    pending.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let (serve, pending, token) = (serve.clone(), pending.clone(), token.clone());
                let _ = std::thread::Builder::new()
                    .name("phitex-attached".into())
                    .spawn(move || {
                        let out = s.try_clone();
                        let got = first(s, &token);
                        pending.fetch_sub(1, Ordering::AcqRel);
                        if let (Some((m, r)), Ok(out)) = (got, out) {
                            let _ = out.set_write_timeout(Some(WRITE_WITHIN));
                            let _ = out.set_nodelay(true);
                            serve(m, r, out);
                        }
                    });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The discovery file written again with `info` (`{pid, main, root,
/// viewer, version}`), the socket's `lsp` and the start added: 0600,
/// whole or not at all (written beside it, then renamed). Nothing before
/// [`open`].
pub fn publish(info: &Value) {
    let Some(p) = PUBLISHED.get() else {
        return;
    };
    let mut v = info.clone();
    v["lsp"] = p.lsp.clone();
    v["started"] = json!(p.started);
    let _w = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let tmp = p.file.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let written = o
        .open(&tmp)
        .and_then(|mut f| f.write_all(format!("{v}\n").as_bytes()));
    if written.is_ok() {
        let _ = std::fs::rename(&tmp, &p.file);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// This watch's discovery file removed (it ends).
pub fn forget() {
    if let Some(p) = PUBLISHED.get() {
        let _ = std::fs::remove_file(&p.file);
    }
}

/// Whether the watch a discovery file names runs: its process (where the
/// system lists them, `/proc`) and its socket, which answers a
/// connection.
pub fn alive(v: &Value) -> bool {
    let Some(pid) = v["pid"].as_u64() else {
        return false;
    };
    if Path::new("/proc/self").exists() && !Path::new(&format!("/proc/{pid}")).exists() {
        return false;
    }
    let Some(port) = v["lsp"]["port"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
    else {
        return false;
    };
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(300),
    )
    .is_ok()
}

/// The running watches' discovery files in `d`, oldest first; those of
/// watches that do not run removed.
pub fn live_in(d: &Path) -> Vec<Value> {
    let mut found: Vec<(PathBuf, Value)> = std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| {
            let v = serde_json::from_slice::<Value>(&std::fs::read(&p).ok()?).ok()?;
            Some((p, v))
        })
        .collect();
    found.retain(|(p, v)| {
        let ours = Some(p.as_path()) == PUBLISHED.get().map(|x| x.file.as_path());
        let keep = ours || alive(v);
        if !keep {
            let _ = std::fs::remove_file(p);
        }
        keep
    });
    found.sort_by(|a, b| a.1["started"].as_str().cmp(&b.1["started"].as_str()));
    found.into_iter().map(|(_, v)| v).collect()
}

/// `phitex sessions`: the running watches, a line each, or (`--json`)
/// their discovery files as one JSON array on a line.
pub fn sessions(ui: &crate::ui::Ui, as_json: bool) -> i32 {
    let d = match dir() {
        Ok(d) => d,
        Err(e) => {
            ui.error(&format!("the sessions folder: {e}"));
            return crate::EXIT_FAILED;
        }
    };
    let live = live_in(&d);
    let mut o = std::io::stdout().lock();
    if as_json {
        let _ = writeln!(o, "{}", Value::Array(live));
        return 0;
    }
    if live.is_empty() {
        ui.line("no phitex watch is running");
    }
    for v in &live {
        let _ = writeln!(
            o,
            "{}  {}  pid {}  lsp {}:{}{}",
            v["main"].as_str().unwrap_or("(starting)"),
            v["started"].as_str().unwrap_or(""),
            v["pid"],
            v["lsp"]["host"].as_str().unwrap_or(""),
            v["lsp"]["port"],
            v["viewer"]
                .as_str()
                .map(|u| format!("  viewer {u}"))
                .unwrap_or_default()
        );
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_their_comparison() {
        let (a, b) = (token(), token());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(same(a.as_bytes(), a.clone().as_bytes()));
        assert!(!same(a.as_bytes(), b.as_bytes()));
        assert!(!same(a.as_bytes(), &a.as_bytes()[..31]));
        assert!(!same(b"", a.as_bytes()));
    }

    fn framed(v: &Value) -> Vec<u8> {
        let body = v.to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    /// What [`first`] makes of a connection that sends `bytes` (and then
    /// waits, open).
    fn handshake(bytes: Vec<u8>) -> Option<Value> {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port");
        let at = l.local_addr().expect("its address");
        let c = std::thread::spawn(move || {
            let mut s = TcpStream::connect(at).expect("connected");
            let _ = s.write_all(&bytes);
            std::thread::sleep(FIRST_WITHIN + Duration::from_millis(500));
        });
        let (s, _) = l.accept().expect("a connection");
        let got = first(s, "0123456789abcdef0123456789abcdef").map(|(m, _)| m);
        drop(c);
        got
    }

    /// Only `initialize` with the token opens a connection: a wrong or
    /// missing token, another method first, a message past the bound in
    /// bytes, or none within the bound in time are refused.
    #[test]
    fn the_first_message_must_bring_the_token() {
        let init = |token: Value| {
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "capabilities": {}, "initializationOptions": {"token": token}}})
        };
        let good = init(json!("0123456789abcdef0123456789abcdef"));
        assert_eq!(handshake(framed(&good)), Some(good));
        assert_eq!(
            handshake(framed(&init(json!("0123456789abcdef0123456789abcdee")))),
            None
        );
        assert_eq!(handshake(framed(&init(Value::Null))), None);
        let mut other = init(json!("0123456789abcdef0123456789abcdef"));
        other["method"] = json!("shutdown");
        assert_eq!(handshake(framed(&other)), None);
        let mut big = init(json!("0123456789abcdef0123456789abcdef"));
        big["params"]["padding"] = json!("x".repeat(FIRST_MAX));
        assert_eq!(handshake(framed(&big)), None);
        let t = Instant::now();
        assert_eq!(handshake(b"Content-Length: 10\r\n\r\n{".to_vec()), None);
        assert!(t.elapsed() < FIRST_WITHIN + Duration::from_secs(2));
    }

    /// A discovery file whose watch does not run (its process gone, its
    /// port closed) is not listed, and is removed; a running one is.
    #[test]
    fn stale_discovery_files_are_ignored_and_removed() {
        let d = std::env::temp_dir().join(format!("phitex-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("a folder");
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port");
        let port = l.local_addr().expect("its address").port();
        let closed = {
            let x = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port");
            x.local_addr().expect("its address").port()
        };
        let running = json!({"pid": std::process::id(), "main": "/p/a.tex",
            "lsp": {"host": "127.0.0.1", "port": port, "token": "t"}, "started": "2026-11-09T10:00:00Z"});
        let gone = json!({"pid": u32::MAX, "main": "/p/b.tex",
            "lsp": {"host": "127.0.0.1", "port": port, "token": "t"}, "started": "2026-11-09T09:00:00Z"});
        let shut = json!({"pid": std::process::id(), "main": "/p/c.tex",
            "lsp": {"host": "127.0.0.1", "port": closed, "token": "t"}, "started": "2026-11-09T08:00:00Z"});
        for (n, v) in [("1.json", &running), ("2.json", &gone), ("3.json", &shut)] {
            std::fs::write(d.join(n), v.to_string()).expect("written");
        }
        std::fs::write(d.join("4.json"), "{not json").expect("written");
        let live = live_in(&d);
        assert_eq!(live, [running]);
        assert!(d.join("1.json").exists());
        if Path::new("/proc/self").exists() {
            assert!(!d.join("2.json").exists(), "a dead process's file removed");
        }
        assert!(!d.join("3.json").exists(), "a closed socket's file removed");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn the_folder_is_the_users_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = std::env::temp_dir().join(format!("phitex-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("a folder");
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).expect("its mode");
        private(&d).expect("ours");
        let mode = std::fs::metadata(&d).expect("it").permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&d);
        // (a folder another user owns: refused)
        if own_uid() != Some(0) && Path::new("/").exists() {
            assert!(private(Path::new("/")).is_err());
        }
    }
}
