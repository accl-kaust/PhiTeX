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
/// The nightly's key (nightly.yml signs with it): a nightly is never
/// checked against the release key, nor a release against this one.
const NIGHTLY_PUBKEY: &str = include_str!("../../../ci/nightly.pub");
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

/// `a` is a later version than `b` (`1.61` after `1.6.9`: φ's digits,
/// the minor growing; `1.6.1` after `1.6`: an update; a missing part is
/// 0). A nightly (`1.6.1-nightly.202611010230`, nightly.yml) comes after
/// the version before it and before the one it leads to; nightlies by
/// their build's time.
fn later(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> (Vec<u64>, Option<u64>) {
        let v = v.trim_start_matches('v');
        let (n, pre) = v.split_once('-').map_or((v, None), |(n, p)| (n, Some(p)));
        let nums = n.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        let built = pre.map(|p| {
            p.rsplit('.')
                .next()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0)
        });
        (nums, built)
    };
    let ((a, pa), (b, pb)) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    match (pa, pb) {
        (None, Some(_)) => true,
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
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

/// A release to upgrade to: its version, its tag, and the key its
/// `SHA256SUMS` is signed with.
struct Release {
    version: String,
    tag: String,
    key: &'static str,
}

/// The latest release (GitHub's `releases/latest`: the newest not marked
/// a pre-release).
fn latest() -> Result<Release, String> {
    let b = get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))?;
    let v: Value = serde_json::from_slice(&b).map_err(|e| format!("GitHub's answer: {e}"))?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or_else(|| "GitHub's answer names no release".to_owned())?;
    Ok(Release {
        version: tag.trim_start_matches('v').to_owned(),
        tag: tag.to_owned(),
        key: PUBKEY,
    })
}

/// The nightly (the pre-release tagged `nightly`, its version the last
/// word of its title, `PhiTeX nightly 1.6.1-nightly.202611010230`).
fn nightly() -> Result<Release, String> {
    let b = get(&format!(
        "https://api.github.com/repos/{REPO}/releases/tags/nightly"
    ))?;
    let v: Value = serde_json::from_slice(&b).map_err(|e| format!("GitHub's answer: {e}"))?;
    let version = v["name"]
        .as_str()
        .and_then(|n| n.split_whitespace().last())
        .filter(|n| n.contains("-nightly."))
        .ok_or_else(|| "GitHub's nightly names no version".to_owned())?;
    Ok(Release {
        version: version.to_owned(),
        tag: "nightly".to_owned(),
        key: NIGHTLY_PUBKEY,
    })
}

/// What `phitex upgrade` follows: the user's `update-channel`, else what
/// install.sh recorded (`PHITEX_CHANNEL=nightly`), else `stable`.
fn channel() -> Result<&'static str, String> {
    let set = crate::config::user().ok().and_then(|c| c.update_channel);
    match set.as_deref() {
        Some("stable") => Ok("stable"),
        Some("nightly") => Ok("nightly"),
        Some(other) => Err(format!(
            "update-channel is '{other}': it is \"stable\" or \"nightly\" (phitex config update-channel stable)"
        )),
        None if install_record().is_some_and(|r| r["channel"] == "nightly") => Ok("nightly"),
        None => Ok("stable"),
    }
}

/// The release `phitex upgrade` takes on `channel`: the latest stable
/// one; on `nightly`, the nightly unless a stable release is later.
fn newest(channel: &str) -> Result<Release, String> {
    let stable = latest();
    if channel != "nightly" {
        return stable;
    }
    match (stable, nightly()) {
        (Ok(s), Ok(n)) => Ok(if later(&s.version, &n.version) { s } else { n }),
        (Ok(s), Err(_)) => Ok(s),
        (Err(_), n) => n,
    }
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

/// Release `r`'s binary for this machine, verified: `SHA256SUMS` signed
/// by its channel's key built in, the tarball's hash in it.
fn fetch(r: &Release, asset: &str) -> Result<Vec<u8>, String> {
    let base = format!("https://github.com/{REPO}/releases/download/{}", r.tag);
    let sums = get(&format!("{base}/SHA256SUMS"))?;
    let sig = get(&format!("{base}/SHA256SUMS.minisig"))?;
    let pk =
        minisign_verify::PublicKey::decode(r.key).map_err(|e| format!("the key built in: {e}"))?;
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

/// `phitex upgrade`: the exit status.
pub fn upgrade(ui: &Ui) -> i32 {
    let have = crate::version();
    let r = match channel().and_then(|c| newest(c).map_err(|e| format!("no release found: {e}"))) {
        Ok(r) => r,
        Err(e) => {
            ui.error(&e);
            return crate::EXIT_FAILED;
        }
    };
    let v = r.version.clone();
    if !later(&v, have) {
        ui.line(&format!(
            "phitex {have} is the latest {}",
            if r.tag == "nightly" {
                "nightly"
            } else {
                "release"
            }
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
    match fetch(&r, &asset).and_then(|b| replace(&exe, &b)) {
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

/// The update check here.
#[derive(PartialEq, Eq)]
enum Setting {
    /// Not a terminal, CI, or `PHITEX_NO_UPDATE_CHECK`: never a word.
    Mute,
    /// The user's `update-check` not set yet.
    Unset,
    Off,
    On,
}

/// The update check here: muted off a terminal, under CI or
/// `PHITEX_NO_UPDATE_CHECK`, else as the user's `update-check` says.
fn setting() -> Setting {
    use std::io::IsTerminal as _;
    if !std::io::stderr().is_terminal()
        || std::env::var_os("CI").is_some()
        || std::env::var_os("PHITEX_NO_UPDATE_CHECK").is_some()
    {
        return Setting::Mute;
    }
    match crate::config::user().ok().and_then(|c| c.update_check) {
        None => Setting::Unset,
        Some(false) => Setting::Off,
        Some(true) => Setting::On,
    }
}

fn kept() -> PathBuf {
    crate::tex::cache_dir().join("news.json")
}

/// The update check's fetch, as a run starts: `news.json` fetched in the
/// background when the kept one is a day old and the check is on (the
/// run never waits for it; [`check`] reads what is kept when it speaks).
pub fn begin() {
    if setting() != Setting::On {
        return;
    }
    let kept = kept();
    let age = std::fs::metadata(&kept)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    if age.is_some_and(|a| a < Duration::from_hours(24)) {
        return;
    }
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

/// The update check speaking (after a build's result, as a watch
/// starts): with `update-check` not set, a line asking for it, every run;
/// on, what the kept `news.json` says for this version.
pub fn check(ui: &Ui) {
    let st = ui.st;
    match setting() {
        Setting::Mute | Setting::Off => {}
        Setting::Unset => ui.line(&format!(
            "{} {} {}",
            st.dim("update checks are not set:"),
            st.cyan("phitex config update-check on"),
            st.dim("(off: never ask again)")
        )),
        Setting::On => {
            let Some(n) = std::fs::read(kept())
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            else {
                return;
            };
            let by_script = install_record().is_some_and(|r| r["method"] == "script");
            for (k, l) in said(&n, crate::version(), by_script).iter().enumerate() {
                if k == 0 {
                    ui.line(&st.yellow(l));
                } else {
                    ui.line(l);
                }
            }
        }
    }
}

/// What news `n` says to version `v`: the releases after it, each with
/// its summary, the advisories for `v`, and how to update (`phitex
/// upgrade` for an install.sh install); nothing when `v` is the latest
/// and no advisory names it.
fn said(n: &Value, v: &str, by_script: bool) -> Vec<String> {
    if n["schema"] != 1 {
        return Vec::new();
    }
    let entries = n["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let newer = entries
        .iter()
        .filter(|e| e["kind"] == "release")
        .filter_map(|e| Some((e["version"].as_str()?, e["summary"].as_str().unwrap_or(""))))
        .filter(|(r, _)| later(r, v))
        .map(|(r, s)| format!("  {r:<7} {s}"));
    let advisories: Vec<String> = entries
        .iter()
        .filter(|e| e["kind"] == "advisory")
        .filter_map(|e| Some((e["affects"].as_str()?, e["summary"].as_str()?)))
        .filter(|(r, _)| affects(r, v))
        .map(|(_, s)| format!("  ⚠ {s}"))
        .collect();
    let latest = n["latest"].as_str().filter(|l| later(l, v));
    if latest.is_none() && advisories.is_empty() {
        return Vec::new();
    }
    let mut out = vec![match latest {
        Some(l) => format!("phitex {l} is out (you have {v}):"),
        None => format!("an advisory for phitex {v}:"),
    }];
    out.extend(newer);
    out.extend(advisories);
    if latest.is_some() {
        out.push(if by_script {
            "update: phitex upgrade".to_owned()
        } else {
            "update: the way you installed phitex (curl -fsSL https://phitex.org/install.sh | sh for the script's)".to_owned()
        });
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
        // (a milestone's updates: 1.6.1 … 1.6.10, then the next milestone)
        assert!(later("1.6.1", "1.6"));
        assert!(later("1.6.10", "1.6.9"));
        assert!(later("1.61", "1.6.10"));
        assert!(!later("1.6.2", "1.61"));
        assert!(affects("<1.6.2", "1.6.1"));
        assert!(affects("<1.618", "1.61"));
        assert!(!affects("<1.618", "1.618"));
        assert!(affects(">=1.61", "1.618"));
        assert!(affects("1.6", "1.6.0"));
    }

    #[test]
    fn news_said() {
        let n = serde_json::json!({"schema": 1, "latest": "1.618", "entries": [
            {"kind": "release", "version": "1.618", "summary": "SyncTeX for XeLaTeX."},
            {"kind": "release", "version": "1.61", "summary": "Collaborators' cursors."},
            {"kind": "advisory", "affects": "<1.61", "summary": "1.6 can write a wrong PDF."},
            {"kind": "notice", "summary": "ignored"}]});
        assert_eq!(
            said(&n, "1.6", true),
            [
                "phitex 1.618 is out (you have 1.6):",
                "  1.618   SyncTeX for XeLaTeX.",
                "  1.61    Collaborators' cursors.",
                "  ⚠ 1.6 can write a wrong PDF.",
                "update: phitex upgrade"
            ]
        );
        assert_eq!(said(&n, "1.61", true).len(), 3);
        assert_eq!(said(&n, "1.618", true), Vec::<String>::new());
        assert_eq!(
            said(
                &serde_json::json!({"schema": 2, "latest": "9.9"}),
                "1.6",
                true
            ),
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
        assert!(minisign_verify::PublicKey::decode(NIGHTLY_PUBKEY).is_ok());
        assert_ne!(PUBKEY.trim(), NIGHTLY_PUBKEY.trim());
    }

    #[test]
    fn nightlies_between_versions() {
        let n = "1.6.1-nightly.202611010230";
        assert!(later(n, "1.6"));
        assert!(later(n, "1.6.0"));
        assert!(!later(n, "1.6.1"));
        assert!(later("1.6.1", n));
        assert!(later("1.6.1-nightly.202611020230", n));
        assert!(!later(n, n));
        assert!(later("1.0.1-nightly.202610310230", "1.0"));
        assert!(later("1.0", "1.0.0-nightly.202610310230"));
        assert!(later("1.0.0-nightly.202610310230", "0.9"));
        assert!(later("1.6", "1.0.1-nightly.202610310230"));
    }
}
