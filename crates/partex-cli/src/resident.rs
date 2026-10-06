//! `-resident` (or `PARTEX_RESIDENT=1`): a plain invocation that rebuilds
//! incrementally. The first one starts a resident session in the
//! background (this program again, in its own process group) and asks
//! it to build; later ones with the same program, arguments, directory
//! and TeX environment ask the same session, which rebuilds from its
//! checkpoints like `-watch` (DESIGN.md §7.0). The terminal transcript
//! and exit status come back as a plain run's would. The session serves
//! one request at a time, and ends after an idle while
//! (`PARTEX_RESIDENT_IDLE`, in seconds; half an hour by default).
//!
//! Like `-watch`, the session keeps the clock it started with and runs
//! in `nonstopmode` unless told otherwise (it has no terminal); a new
//! day starts a new session unless `SOURCE_DATE_EPOCH` fixes the date.
//! Any failure to reach a session falls back to running here.

use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use partex_core::Params;

use crate::native::NativeHost;
use crate::session::Session;

/// Set (to the socket path) in the environment of a resident session.
const SERVE: &str = "PARTEX_RESIDENT_SERVE";

/// Whether this invocation asks for a resident session (and is not one).
pub fn wanted() -> bool {
    std::env::var_os(SERVE).is_none()
        && (std::env::var_os("PARTEX_RESIDENT").is_some_and(|v| !v.is_empty() && v != "0")
            || crate::args()
                .iter()
                .skip(1)
                .any(|a| a == "-resident" || a == "--resident"))
        && !crate::args()
            .iter()
            .skip(1)
            .any(|a| a == "-watch" || a == "--watch")
}

/// The socket of the session this invocation belongs to, if it is one.
pub fn serving() -> Option<PathBuf> {
    std::env::var_os(SERVE).map(PathBuf::from)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The environment a job's result may depend on (beyond its files):
/// TeX's and kpathsea's variables, partex's own, and the clock's.
pub fn job_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut env: Vec<_> = std::env::vars_os()
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            (k.starts_with("TEX")
                || k.starts_with("KPSE")
                || k.starts_with("PARTEX_")
                || matches!(
                    &*k,
                    "SOURCE_DATE_EPOCH" | "FORCE_SOURCE_DATE" | "TZ" | "HOME"
                ))
                && !k.starts_with("PARTEX_RESIDENT")
        })
        .collect();
    env.sort();
    env
}

/// Where the session for this invocation listens.
pub fn socket_path() -> PathBuf {
    let mut h = std::hash::DefaultHasher::new();
    let exe = std::env::current_exe().unwrap_or_default();
    exe.hash(&mut h);
    std::fs::metadata(&exe)
        .and_then(|m| m.modified())
        .ok()
        .hash(&mut h);
    std::env::current_dir().unwrap_or_default().hash(&mut h);
    std::env::args_os().collect::<Vec<_>>().hash(&mut h);
    let env = job_env();
    env.hash(&mut h);
    if std::env::var_os("SOURCE_DATE_EPOCH").is_none() {
        (now_secs() / 86_400).hash(&mut h);
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
        || {
            let user = std::env::var("USER").unwrap_or_default();
            std::env::temp_dir().join(format!("phitex-{user}"))
        },
        |d| PathBuf::from(d).join("phitex"),
    );
    dir.join(format!("{:016x}.sock", h.finish()))
}

/// Ask the session at `path` (started if there is none) to build: the
/// exit status, after writing the terminal transcript; `None` if no
/// session could be reached.
pub fn client(path: &Path) -> Option<i32> {
    let mut stream = if let Ok(s) = UnixStream::connect(path) {
        s
    } else {
        // (a session that ended leaves its socket behind)
        let _ = std::fs::remove_file(path);
        start(path)?;
        connect_soon(path)?
    };
    stream.write_all(b"build\n").ok()?;
    let mut read = || -> Option<Vec<u8>> {
        let mut len = [0; 8];
        stream.read_exact(&mut len).ok()?;
        let mut b = vec![0; usize::try_from(u64::from_le_bytes(len)).ok()?];
        stream.read_exact(&mut b).ok()?;
        Some(b)
    };
    let (term, report, status) = (read()?, read()?, read()?);
    let mut out = std::io::stdout().lock();
    out.write_all(&term).ok()?;
    out.flush().ok()?;
    // (`PARTEX_REPORT`: what the session did, as `-watch` says)
    if std::env::var_os("PARTEX_REPORT").is_some() {
        eprintln!("{}", String::from_utf8_lossy(&report));
    }
    Some(i32::from(*status.first()?))
}

/// Start a session for this invocation, listening at `path`.
fn start(path: &Path) -> Option<()> {
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).ok()?;
    // (under the name it was invoked by, which picks the command line:
    // `compat.rs`)
    std::process::Command::new(std::env::current_exe().ok()?)
        .arg0(std::env::args_os().next()?)
        .args(std::env::args_os().skip(1))
        .env(SERVE, path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // (its own group: a closed terminal does not end it)
        .process_group(0)
        .spawn()
        .ok()?;
    Some(())
}

/// Connect to a session being started (it listens before it builds).
fn connect_soon(path: &Path) -> Option<UnixStream> {
    for _ in 0..600 {
        if let Ok(s) = UnixStream::connect(path) {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

/// Be the session listening at `path`: build on the first request,
/// rebuild on each later one.
pub fn serve(
    mut params: Params,
    command_line: Vec<u8>,
    host: NativeHost,
    every: u64,
    path: &Path,
) -> ! {
    if params.interaction.is_none() {
        params.interaction = Some(1);
    }
    let _ = std::fs::remove_file(path);
    let Ok(listener) = UnixListener::bind(path) else {
        std::process::exit(1);
    };
    let idle = std::env::var("PARTEX_RESIDENT_IDLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1800);
    let last = Arc::new(AtomicU64::new(now_secs()));
    {
        let (last, path) = (last.clone(), path.to_owned());
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if now_secs().saturating_sub(last.load(Ordering::Relaxed)) > idle {
                    let _ = std::fs::remove_file(&path);
                    std::process::exit(0);
                }
            }
        });
    }
    let converge = crate::converge_wanted();
    let mut s = Session::new(params, command_line, host, every);
    let mut history = None;
    let mut between = crate::Between::default();
    for conn in listener.incoming() {
        let Ok(mut conn) = conn else { continue };
        let mut line = String::new();
        if BufReader::new(&conn).read_line(&mut line).is_err() {
            continue;
        }
        let (reports, term, h) = crate::serve_request(&mut s, history, converge, &mut between);
        history = Some(h);
        let report = reports.join("\n");
        let mut reply = Vec::new();
        for part in [&term[..], report.as_bytes(), &[u8::from(h > 1)]] {
            reply.extend_from_slice(&u64::try_from(part.len()).unwrap_or(0).to_le_bytes());
            reply.extend_from_slice(part);
        }
        let _ = conn.write_all(&reply);
        last.store(now_secs(), Ordering::Relaxed);
    }
    std::process::exit(0);
}
