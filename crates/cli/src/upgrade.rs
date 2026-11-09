//! `phitex upgrade` and the opt-in update check (docs/spec-distribution.md
//! §4).
//!
//! - **upgrade**: for an install made by phitex.org/install.sh (its
//!   `install.json`), the latest GitHub release's tarball for this machine,
//!   checked as install.sh checks it, and more: `SHA256SUMS`' minisign
//!   signature against the key built in (`ci/minisign.pub`), always, then
//!   the tarball against `SHA256SUMS`. The binary is replaced by rename
//!   (written beside it first: a running phitex keeps its file). Another
//!   install method gets the command for it instead. On Windows the
//!   same for phitex.org/install.ps1's install, its zip's `phitex.exe`,
//!   the running `.exe` renamed aside (Windows renames a running program's
//!   file but does not replace it) before the new one takes its name.
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
/// CI's test of `phitex upgrade` (checks.yml's texlive job, on Windows): a
/// copy of a release served from the runner (`BASE/latest.json`,
/// `BASE/download/TAG/…`), signed with a key made there (its base64
/// line). Compiled in only by that test's build: a release's and a
/// nightly's set neither, and nothing at run time can.
const TEST_BASE: Option<&str> = option_env!("PHITEX_UPGRADE_TEST_BASE");
const TEST_KEY: Option<&str> = option_env!("PHITEX_UPGRADE_TEST_KEY");
const NEWS: [&str; 2] = [
    "https://phitex.org/news.json",
    "https://phitex-org.pages.dev/news.json",
];

/// The installer of this system's builds, the command that runs it.
const INSTALLER: (&str, &str) = if cfg!(windows) {
    (
        "phitex.org/install.ps1",
        "irm https://phitex.org/install.ps1 | iex",
    )
} else {
    (
        "phitex.org/install.sh",
        "curl -fsSL https://phitex.org/install.sh | sh",
    )
};

/// This machine's release tarball, as install.sh names it (Windows's
/// zip, as install.ps1 does).
fn asset() -> Option<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => return windows_asset(std::env::consts::ARCH),
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some(format!("phitex-{arch}-{os}.tar.gz"))
}

/// Windows's release zip for `arch` (x86-64's alone, for now).
fn windows_asset(arch: &str) -> Option<String> {
    (arch == "x86_64").then(|| format!("phitex-{arch}-windows.zip"))
}

/// The file named `name` in zip archive `zip` (stored or deflated), by
/// its central directory.
fn unzip(zip: &[u8], name: &str) -> Option<Vec<u8>> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(zip.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(zip.get(at..at + 4)?.try_into().ok()?));
    let n = |x: u32| usize::try_from(x).ok();
    // (the end record: the last signature with its comment running to the end)
    let end = (0..=zip.len().checked_sub(22)?)
        .rev()
        .find(|&i| u32_at(i) == Some(0x0605_4b50))?;
    let entries = u16_at(end + 10)?;
    let mut at = n(u32_at(end + 16)?)?;
    for _ in 0..entries {
        if u32_at(at)? != 0x0201_4b50 {
            return None;
        }
        let method = u16_at(at + 10)?;
        let packed = n(u32_at(at + 20)?)?;
        let size = n(u32_at(at + 24)?)?;
        let name_len = usize::from(u16_at(at + 28)?);
        let skip = name_len + usize::from(u16_at(at + 30)?) + usize::from(u16_at(at + 32)?);
        let local = n(u32_at(at + 42)?)?;
        if zip.get(at + 46..at + 46 + name_len)? == name.as_bytes() {
            if u32_at(local)? != 0x0403_4b50 {
                return None;
            }
            let data =
                local + 30 + usize::from(u16_at(local + 26)?) + usize::from(u16_at(local + 28)?);
            let body = zip.get(data..data.checked_add(packed)?)?;
            let out = match method {
                0 => body.to_vec(),
                8 => {
                    let mut out = Vec::with_capacity(size);
                    if !phitex_codecs::inflate::inflate_raw(body, &mut out) {
                        return None;
                    }
                    out
                }
                _ => return None,
            };
            return (out.len() == size).then_some(out);
        }
        at += 46 + skip;
    }
    None
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
    key: std::borrow::Cow<'static, str>,
}

/// The latest release (GitHub's `releases/latest`: the newest not marked
/// a pre-release).
fn latest() -> Result<Release, String> {
    let b = get(&TEST_BASE.map_or_else(
        || format!("https://api.github.com/repos/{REPO}/releases/latest"),
        |b| format!("{b}/latest.json"),
    ))?;
    let v: Value = serde_json::from_slice(&b).map_err(|e| format!("GitHub's answer: {e}"))?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or_else(|| "GitHub's answer names no release".to_owned())?;
    Ok(Release {
        version: tag.trim_start_matches('v').to_owned(),
        tag: tag.to_owned(),
        key: TEST_KEY.map_or(std::borrow::Cow::Borrowed(PUBKEY), |k| {
            std::borrow::Cow::Owned(format!("untrusted comment: test key\n{k}\n"))
        }),
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
        key: std::borrow::Cow::Borrowed(NIGHTLY_PUBKEY),
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
    let base = TEST_BASE.map_or_else(
        || format!("https://github.com/{REPO}/releases/download/{}", r.tag),
        |b| format!("{b}/download/{}", r.tag),
    );
    let sums = get(&format!("{base}/SHA256SUMS"))?;
    let sig = get(&format!("{base}/SHA256SUMS.minisig"))?;
    let pk =
        minisign_verify::PublicKey::decode(&r.key).map_err(|e| format!("the key built in: {e}"))?;
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
    if asset.ends_with(".zip") {
        return unzip(&tgz, "phitex.exe").ok_or_else(|| format!("{asset} has no phitex.exe"));
    }
    let tar = phitex_shelf::gunzip(&tgz).ok_or_else(|| format!("{asset}: not gzip"))?;
    untar(&tar, "phitex")
        .map(<[u8]>::to_vec)
        .ok_or_else(|| format!("{asset} has no phitex binary"))
}

/// Where install.sh says it installed (`$XDG_DATA_HOME/phitex/install.json`;
/// install.ps1: `%LOCALAPPDATA%\phitex\install.json`, [`crate::dirs`]).
fn install_record() -> Option<Value> {
    let d = crate::dirs::phitex(crate::dirs::Kind::Data)?;
    serde_json::from_slice(&std::fs::read(d.join("install.json")).ok()?).ok()
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
    #[cfg(windows)]
    {
        // (the running program's file renamed aside, never removed while
        // it runs; earlier upgrades' removed now, those still running kept)
        let dir = exe.parent().unwrap_or(Path::new("."));
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let n = e.file_name();
                let n = n.to_string_lossy();
                if n.starts_with(".phitex-") && n.ends_with(".old") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let old = exe.with_file_name(format!(".phitex-{}.old", std::process::id()));
        std::fs::rename(exe, &old).map_err(|e| format!("{}: {e}", exe.display()))?;
        if let Err(e) = std::fs::rename(&new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(format!("{}: {e}", exe.display()));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    std::fs::rename(&new, exe).map_err(|e| format!("{}: {e}", exe.display()))
}

/// `phitex upgrade`: the newest release (or nightly) checked for, and, if
/// it is later, installed once the user says yes ("[Y/n]": Enter is yes),
/// or at once with `--no-confirm`. Off a terminal it asks nothing and
/// installs nothing without `--no-confirm`. The exit status.
pub fn upgrade(ui: &Ui, no_confirm: bool) -> i32 {
    // (a source build's version names the release it leads to: any
    // release or nightly is offered to it)
    let have = if crate::source_build() {
        format!("{} (source build)", crate::version())
    } else {
        crate::version().to_owned()
    };
    let r = match channel().and_then(|c| newest(c).map_err(|e| format!("no release found: {e}"))) {
        Ok(r) => r,
        Err(e) => {
            ui.error(&e);
            return crate::EXIT_INTERNAL;
        }
    };
    let v = r.version.clone();
    if !crate::source_build() && !later(&v, crate::version()) {
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
            "phitex {v} is out (you have {have}); this phitex was not installed by {}: \
             update it the way it was installed ({}, cargo install, or your package manager)",
            INSTALLER.0, INSTALLER.1
        ));
        return crate::EXIT_USAGE;
    };
    if !no_confirm {
        use std::io::IsTerminal as _;
        if !std::io::stdin().is_terminal() {
            ui.line(&format!(
                "phitex {v} is out (you have {have}); run phitex upgrade --no-confirm to install it without asking"
            ));
            return 0;
        }
        ui.ask(&format!("upgrade phitex {have} to {v}? [Y/n]"));
        let mut a = String::new();
        let read = std::io::stdin().read_line(&mut a);
        // (the line typed ended it already; an end of input did not)
        if matches!(read, Ok(0)) {
            ui.answered(None);
        }
        if read.is_err() || !yes(&a, true) {
            ui.line(&format!("phitex {have} kept"));
            return 0;
        }
    }
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
            crate::EXIT_INTERNAL
        }
    }
}

/// Whether answer `a` to a "[Y/n]" (`default` true) or "[y/N]" question
/// is yes: y or yes in any case, else the default when it is empty.
fn yes(a: &str, default: bool) -> bool {
    match a.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        _ => false,
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
/// starts): with `update-check` not set, a line asking for it, once a
/// version;
/// on, what the kept `news.json` says for this version.
pub fn check(ui: &Ui) {
    let st = ui.st;
    match setting() {
        Setting::Mute | Setting::Off => {}
        // (asked once a version: a new one asks again, never every run)
        Setting::Unset if asked_before(crate::version()) => {}
        Setting::Unset => ui.line(&format!(
            "{} {} {}",
            st.dim("Check daily for new PhiTeX releases?"),
            st.cyan("phitex config update-check on"),
            st.dim("(or off; this note shows once per version)")
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

/// Whether the update check's question was put to version `v` already
/// (`$XDG_STATE_HOME/phitex/update-asked` holds the last version asked);
/// if not, it is kept as asked now. A state folder that can't be written
/// asks nothing: a note every run is worse than none.
fn asked_before(v: &str) -> bool {
    let env = |k| std::env::var_os(k).filter(|d| !d.is_empty());
    let Some(d) = env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))
    else {
        return true;
    };
    let f = d.join("phitex").join("update-asked");
    if std::fs::read_to_string(&f).is_ok_and(|s| s.trim() == v) {
        return true;
    }
    let kept = std::fs::create_dir_all(d.join("phitex")).is_ok() && std::fs::write(&f, v).is_ok();
    !kept
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
            format!(
                "update: the way you installed phitex ({} for the script's)",
                INSTALLER.1
            )
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers() {
        for (a, d, want) in [
            ("\n", true, true),
            ("", true, true),
            ("y\n", true, true),
            ("Yes\n", true, true),
            (" Y ", false, true),
            ("n\n", true, false),
            ("no\n", true, false),
            ("\n", false, false),
            ("maybe\n", true, false),
        ] {
            assert_eq!(yes(a, d), want, "{a:?} default {d}");
        }
    }

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

    /// A zip's file read by its central directory, stored or deflated
    /// (a fixed deflate block of `hello`), and Windows's asset name.
    #[test]
    fn zip() {
        let entry = |name: &str, method: u16, body: &[u8], size: u32, at: u32| {
            let mut local = vec![0x50, 0x4b, 3, 4, 20, 0, 0, 0];
            local.extend(method.to_le_bytes());
            local.extend([0; 8]); // (time, date, crc: not read)
            local.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
            local.extend(size.to_le_bytes());
            local.extend(u16::try_from(name.len()).unwrap().to_le_bytes());
            local.extend([0, 0]);
            local.extend(name.as_bytes());
            local.extend(body);
            let mut central = vec![0x50, 0x4b, 1, 2, 20, 0, 20, 0, 0, 0];
            central.extend(method.to_le_bytes());
            central.extend([0; 8]);
            central.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
            central.extend(size.to_le_bytes());
            central.extend(u16::try_from(name.len()).unwrap().to_le_bytes());
            central.extend([0; 12]);
            central.extend(at.to_le_bytes());
            central.extend(name.as_bytes());
            (local, central)
        };
        // (`hello`, deflated: one fixed-Huffman block)
        let deflated = [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00];
        let (l1, c1) = entry("README", 0, b"stored", 6, 0);
        let (l2, c2) = entry(
            "phitex.exe",
            8,
            &deflated,
            5,
            u32::try_from(l1.len()).unwrap(),
        );
        let mut z = [l1.as_slice(), &l2].concat();
        let cd = u32::try_from(z.len()).unwrap();
        z.extend(&c1);
        z.extend(&c2);
        z.extend([0x50, 0x4b, 5, 6, 0, 0, 0, 0, 2, 0, 2, 0]);
        z.extend(u32::try_from(c1.len() + c2.len()).unwrap().to_le_bytes());
        z.extend(cd.to_le_bytes());
        z.extend([0, 0]);
        assert_eq!(unzip(&z, "phitex.exe").as_deref(), Some(&b"hello"[..]));
        assert_eq!(unzip(&z, "README").as_deref(), Some(&b"stored"[..]));
        assert_eq!(unzip(&z, "other"), None);
        assert_eq!(unzip(&z[..z.len() - 3], "phitex.exe"), None);
        assert_eq!(
            windows_asset("x86_64").as_deref(),
            Some("phitex-x86_64-windows.zip")
        );
        assert_eq!(windows_asset("aarch64"), None);
    }

    /// Windows: the program replaced by renaming the old aside (a running
    /// program's file can be renamed, not replaced), the one left aside
    /// by the upgrade before removed now.
    #[test]
    #[cfg(windows)]
    fn replaced_aside() {
        let dir = std::env::temp_dir().join(format!("phitex-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("phitex.exe");
        std::fs::write(&exe, b"one").unwrap();
        replace(&exe, b"two").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"two");
        replace(&exe, b"three").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"three");
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left.len(), 2, "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
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
