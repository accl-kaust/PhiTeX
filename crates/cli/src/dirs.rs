//! The user's directories for phitex's files: XDG's (`$XDG_CONFIG_HOME`
//! and the others, else under the home directory) on Unix; on Windows
//! the same variables where set, else the known folders, `%APPDATA%`
//! (settings, which roam with the user) and `%LOCALAPPDATA%` (the rest).

use std::path::PathBuf;

/// What a directory holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Settings (`phitex.toml`).
    Config,
    /// What can be made again (`phitex clean --all` empties it).
    Cache,
    /// What phitex remembers (the news last shown).
    State,
    /// What an installer records (`install.json`).
    Data,
}

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The home directory: `HOME`, else (Windows) `USERPROFILE`; a folder
/// that is there.
pub fn home() -> Option<PathBuf> {
    env("HOME")
        .or_else(|| {
            if cfg!(windows) {
                env("USERPROFILE")
            } else {
                None
            }
        })
        .filter(|h| h.is_dir())
}

/// phitex's directory of kind `k`: `$XDG_…_HOME/phitex`, else
/// `~/.config/phitex`, `~/.cache/phitex`, `~/.local/state/phitex`,
/// `~/.local/share/phitex`; on Windows `%APPDATA%\phitex` for settings,
/// else `%LOCALAPPDATA%\phitex` (data), `…\phitex\cache`, `…\phitex\state`.
pub fn phitex(k: Kind) -> Option<PathBuf> {
    let xdg = match k {
        Kind::Config => "XDG_CONFIG_HOME",
        Kind::Cache => "XDG_CACHE_HOME",
        Kind::State => "XDG_STATE_HOME",
        Kind::Data => "XDG_DATA_HOME",
    };
    if let Some(d) = env(xdg) {
        return Some(d.join("phitex"));
    }
    if cfg!(windows) {
        let d = match k {
            Kind::Config => env("APPDATA")?.join("phitex"),
            Kind::Data => env("LOCALAPPDATA")?.join("phitex"),
            Kind::Cache => env("LOCALAPPDATA")?.join("phitex").join("cache"),
            Kind::State => env("LOCALAPPDATA")?.join("phitex").join("state"),
        };
        return Some(d);
    }
    let h = home()?;
    Some(
        match k {
            Kind::Config => h.join(".config"),
            Kind::Cache => h.join(".cache"),
            Kind::State => h.join(".local").join("state"),
            Kind::Data => h.join(".local").join("share"),
        }
        .join("phitex"),
    )
}
