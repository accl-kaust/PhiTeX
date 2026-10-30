//! `phitex upgrade` and the opt-in update check (docs/spec-distribution.md
//! §4).
//!
//! - **upgrade**: for an install made by phitex.org/install.sh (its
//!   `install.json`), the latest GitHub release's tarball for this machine,
//!   checked as install.sh checks it, and more: `SHA256SUMS`' minisign
//!   signature against the key built in (`ci/minisign.pub`), always, then
//!   the tarball against `SHA256SUMS`. The binary is replaced by rename
//!   (written beside it first: a running phitex keeps its file). Another
//!   install method gets the command for it instead.
//! - **The update check**: off unless the user's own `phitex.toml` says
//!   `update-check = true`. At most once a day, in the background (never
//!   delaying a build), one GET of `news.json` (phitex.org, then its
//!   pages.dev name), kept in the cache; a newer release, and advisories
//!   for this version, said in one line each from what was kept.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::ui::Ui;

const REPO: &str = "accl-kaust/PhiTeX";
const PUBKEY: &str = include_str!("../../../ci/minisign.pub");
const NEWS: [&str; 2] = [
    "https://phitex.org/news.json",
    "https://phitex-org.pages.dev/news.json",
];

/// This machine's release tarball, as install.sh names it.
fn asset() -> Option<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some(format!("phitex-{arch}-{os}.tar.gz"))
}

/// `a` is a later version than `b` (`1.61` after `1.6`: φ's digits, the
/// minor growing; a missing part is 0).
fn later(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// `v` is in `range`: `<X`, `<=X`, `>=X`, `>X`, `=X` or a bare `X`.
fn affects(range: &str, v: &str) -> bool {
    let r = range.trim();
    if let Some(x) = r.strip_prefix("<=") {
        !later(v, x)
    } else if let Some(x) = r.strip_prefix(">=") {
        !later(x, v)
    } else if let Some(x) = r.strip_prefix('<') {
        later(x, v)
    } else if let Some(x) = r.strip_prefix('>') {
        later(v, x)
    } else {
        let x = r.trim_start_matches('=');
        !later(x, v) && !later(v, x)
    }
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let mut r = crate::shelf::agent()
        .get(url)
        .header("User-Agent", format!("phitex/{}", crate::version()))
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    r.body_mut()
        .with_config()
        .limit(1 << 30)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))
}

/// The latest release's version (GitHub's `releases/latest`: the newest
/// not marked a pre-release).
fn latest() -> Result<String, String> {
    let b = get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))?;
    let v: Value = serde_json::from_slice(&b).map_err(|e| format!("GitHub's answer: {e}"))?;
    v["tag_name"]
        .as_str()
        .map(|t| t.trim_start_matches('v').to_owned())
        .ok_or_else(|| "GitHub's answer names no release".to_owned())
}

/// The file named `name` in tar archive `tar`.
fn untar<'a>(tar: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let mut at = 0;
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            return None;
        }
        let field = |r: std::ops::Range<usize>| {
            let f = &h[r];
            String::from_utf8_lossy(&f[..f.iter().position(|&b| b == 0).unwrap_or(f.len())])
                .trim()
                .to_owned()
        };
        let size = usize::from_str_radix(&field(124..136), 8).ok()?;
        let body = at + 512;
        if field(0..100).trim_start_matches("./") == name && h[156] != b'5' {
            return tar.get(body..body + size);
        }
        at = body + size.div_ceil(512) * 512;
    }
    None
}

/// Release `v`'s binary for this machine, verified: `SHA256SUMS` signed
/// by the key built in, the tarball's hash in it.
fn fetch(v: &str, asset: &str) -> Result<Vec<u8>, String> {
    let base = format!("https://github.com/{REPO}/releases/download/v{v}");
    let sums = get(&format!("{base}/SHA256SUMS"))?;
    let sig = get(&format!("{base}/SHA256SUMS.minisig"))?;
    let pk =
        minisign_verify::PublicKey::decode(PUBKEY).map_err(|e| format!("the key built in: {e}"))?;
    let sig = minisign_verify::Signature::decode(&String::from_utf8_lossy(&sig))
        .map_err(|e| format!("SHA256SUMS.minisig: {e}"))?;
    pk.verify(&sums, &sig, false)
        .map_err(|e| format!("SHA256SUMS' signature does not verify ({e}): nothing installed"))?;
    let want = String::from_utf8_lossy(&sums)
        .lines()
        .find_map(|l| {
            let (h, f) = l.split_once(char::is_whitespace)?;
            (f.trim().trim_start_matches('*') == asset).then(|| h.to_owned())
        })
        .ok_or_else(|| format!("SHA256SUMS does not list {asset}"))?;
    let tgz = get(&format!("{base}/{asset}"))?;
    if phitex_shelf::sha256_hex(&tgz) != want {
        return Err(format!(
            "{asset} does not match SHA256SUMS: nothing installed"
        ));
    }
    let tar = phitex_shelf::gunzip(&tgz).ok_or_else(|| format!("{asset}: not gzip"))?;
    untar(&tar, "phitex")
        .map(<[u8]>::to_vec)
        .ok_or_else(|| format!("{asset} has no phitex binary"))
}

/// Where install.sh says it installed (`$XDG_DATA_HOME/phitex/install.json`).
fn install_record() -> Option<Value> {
    let env = |k| std::env::var_os(k).filter(|d| !d.is_empty());
    let d = env("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))?;
    serde_json::from_slice(&std::fs::read(d.join("phitex").join("install.json")).ok()?).ok()
}

/// `bytes` as the executable `exe`, by rename from beside it.
fn replace(exe: &Path, bytes: &[u8]) -> Result<(), String> {
    let new = exe.with_file_name(".phitex.new");
    std::fs::write(&new, bytes).map_err(|e| format!("{}: {e}", new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", new.display()))?;
    }
    std::fs::rename(&new, exe).map_err(|e| format!("{}: {e}", exe.display()))
}

/// `phitex upgrade [--check]`: the exit status.
pub fn upgrade(ui: &Ui, check: bool) -> i32 {
    let have = crate::version();
    let v = match latest() {
        Ok(v) => v,
        Err(e) => {
            ui.error(&format!("no release found: {e}"));
            return crate::EXIT_FAILED;
        }
    };
    if !later(&v, have) {
        ui.line(&format!("phitex {have} is the latest release"));
        return 0;
    }
    if check {
        ui.line(&format!(
            "phitex {v} is out (you have {have}): phitex upgrade"
        ));
        return 0;
    }
    let exe = std::env::current_exe().ok();
    let by_script = install_record().is_some_and(|r| r["method"] == "script");
    let (Some(exe), true, Some(asset)) = (exe, by_script, asset()) else {
        ui.error(&format!(
            "phitex {v} is out (you have {have}); this phitex was not installed by phitex.org/install.sh: \
             update it the way it was installed (curl -fsSL https://phitex.org/install.sh | sh, \
             cargo install, or your package manager)"
        ));
        return crate::EXIT_FAILED;
    };
    ui.line(&format!("downloading phitex {v} ({asset})"));
    match fetch(&v, &asset).and_then(|b| replace(&exe, &b)) {
        Ok(()) => {
            ui.line(&format!(
                "phitex {v} installed in {} (signature verified)",
                exe.display()
            ));
            0
        }
        Err(e) => {
            ui.error(&e);
            crate::EXIT_FAILED
        }
    }
}

/// The update check (opt-in): what the last fetch of `news.json` says for
/// this version, and a fetch in the background when that is a day old.
pub fn check(ui: &Ui) {
    use std::io::IsTerminal as _;
    let on = crate::config::user()
        .ok()
        .and_then(|c| c.update_check)
        .unwrap_or(false);
    if !on
        || !std::io::stderr().is_terminal()
        || std::env::var_os("CI").is_some()
        || std::env::var_os("PHITEX_NO_UPDATE_CHECK").is_some()
    {
        return;
    }
    let kept = crate::tex::cache_dir().join("news.json");
    let age = std::fs::metadata(&kept)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    if let Ok(b) = std::fs::read(&kept)
        && let Ok(n) = serde_json::from_slice::<Value>(&b)
    {
        for l in said(&n, crate::version()) {
            ui.warning(&l);
        }
    }
    if age.is_none_or(|a| a > Duration::from_hours(24)) {
        // (the next run says what this fetch finds; a process that ends
        // first leaves the kept file as it was)
        std::thread::spawn(move || {
            for u in NEWS {
                if let Ok(b) = get(u)
                    && serde_json::from_slice::<Value>(&b).is_ok()
                {
                    let tmp = kept.with_extension(format!("{}.tmp", std::process::id()));
                    if std::fs::create_dir_all(kept.parent().unwrap_or(Path::new(".")))
                        .and_then(|()| std::fs::write(&tmp, &b))
                        .and_then(|()| std::fs::rename(&tmp, &kept))
                        .is_err()
                    {
                        let _ = std::fs::remove_file(&tmp);
                    }
                    return;
                }
            }
        });
    }
}

/// What news `n` says to version `v`: a newer release, the advisories
/// for `v`.
fn said(n: &Value, v: &str) -> Vec<String> {
    let mut out = Vec::new();
    if n["schema"] != 1 {
        return out;
    }
    if let Some(l) = n["latest"].as_str()
        && later(l, v)
    {
        out.push(format!("phitex {l} is out (you have {v}): phitex upgrade"));
    }
    for e in n["entries"].as_array().into_iter().flatten() {
        if e["kind"] == "advisory"
            && let (Some(r), Some(s)) = (e["affects"].as_str(), e["summary"].as_str())
            && affects(r, v)
        {
            out.push(format!("advisory for phitex {v}: {s}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_by_phi() {
        assert!(later("1.61", "1.6"));
        assert!(later("1.618", "1.61"));
        assert!(later("1.6180", "1.618"));
        assert!(!later("1.6", "1.6.0"));
        assert!(later("1.6", "0.0.0"));
        assert!(affects("<1.618", "1.61"));
        assert!(!affects("<1.618", "1.618"));
        assert!(affects(">=1.61", "1.618"));
        assert!(affects("1.6", "1.6.0"));
    }

    #[test]
    fn news_said() {
        let n = serde_json::json!({"schema": 1, "latest": "1.618", "entries": [
            {"kind": "release", "version": "1.618", "summary": "x"},
            {"kind": "advisory", "affects": "<1.618", "summary": "upgrade: a wrong PDF"},
            {"kind": "notice", "summary": "ignored"}]});
        assert_eq!(said(&n, "1.61").len(), 2);
        assert_eq!(said(&n, "1.618"), Vec::<String>::new());
        assert_eq!(
            said(&serde_json::json!({"schema": 2, "latest": "9.9"}), "1.6"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_file_from_a_tar() {
        let mut tar = vec![0u8; 512];
        tar[..6].copy_from_slice(b"phitex");
        tar[124..135].copy_from_slice(b"00000000005");
        tar.extend_from_slice(b"hello");
        tar.resize(1024 + 1024, 0);
        assert_eq!(untar(&tar, "phitex"), Some(&b"hello"[..]));
        assert_eq!(untar(&tar, "other"), None);
    }

    #[test]
    fn the_key_built_in_reads() {
        assert!(minisign_verify::PublicKey::decode(PUBKEY).is_ok());
    }
}
