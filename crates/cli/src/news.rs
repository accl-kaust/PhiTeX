//! What is new (docs/spec-distribution.md §4): CHANGELOG.md's entry for
//! this version, built into the binary. The first run of a version in a
//! terminal says the entry's summary once; `phitex news` prints the
//! whole entry. No network: it comes with the binary, however installed.
//!
//! An entry is a `## <version> …` heading; its first paragraph is the
//! summary, one or two sentences for the people who just upgraded.

use std::io::IsTerminal as _;
use std::path::PathBuf;

use crate::ui::Ui;

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// `text`'s entry for version `v`: the lines under its `## v` heading,
/// up to the next `## `.
fn entry<'a>(text: &'a str, v: &str) -> Option<&'a str> {
    let mut at = None;
    let mut off = 0;
    for l in text.split_inclusive('\n') {
        if let Some(h) = l.strip_prefix("## ") {
            if let Some(a) = at {
                return Some(text[a..off].trim());
            }
            if h.split_whitespace().next() == Some(v) {
                at = Some(off + l.len());
            }
        }
        off += l.len();
    }
    at.map(|a| text[a..].trim())
}

/// An entry's summary: its first paragraph, on one line.
fn summary(entry: &str) -> String {
    entry
        .lines()
        .take_while(|l| !l.trim().is_empty())
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where the last version whose news was shown is kept:
/// `$XDG_STATE_HOME/phitex/news`, else `~/.local/state/phitex/news`.
fn seen_file() -> Option<PathBuf> {
    let env = |k| std::env::var_os(k).filter(|d| !d.is_empty());
    let d = env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))?;
    Some(d.join("phitex").join("news"))
}

/// `phitex news`: this version's entry, whole.
pub fn print() {
    let v = env!("CARGO_PKG_VERSION");
    match entry(CHANGELOG, v) {
        Some(e) => println!("phitex {v}\n\n{e}"),
        None => println!("phitex {v}: no news for this version"),
    }
}

/// The first run of this version in a terminal: its summary, once (not
/// under CI, `PHITEX_NO_NEWS`, or for a version without an entry).
pub fn once(ui: &Ui) {
    let v = env!("CARGO_PKG_VERSION");
    if !std::io::stderr().is_terminal()
        || std::env::var_os("CI").is_some()
        || std::env::var_os("PHITEX_NO_NEWS").is_some()
    {
        return;
    }
    let (Some(e), Some(f)) = (entry(CHANGELOG, v), seen_file()) else {
        return;
    };
    if std::fs::read_to_string(&f).is_ok_and(|s| s.trim() == v) {
        return;
    }
    // (said only once it is kept as said: an unwritable state directory
    // would say it every run)
    let kept = f
        .parent()
        .is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(&f, format!("{v}\n")).is_ok();
    if kept {
        let st = ui.st;
        ui.line(&format!(
            "{} {} {}",
            st.cyan(&format!("phitex {v}:")),
            summary(e),
            st.dim("(phitex news for more)")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\n\nIntro.\n\n## 1.61 (2027-01-02)\nFaster.\nAnd more.\n\n- one\n\n## 1.6 (2026-11-02)\nFirst.\n\n### Detail\n- two\n";

    #[test]
    fn entries_by_version() {
        assert_eq!(entry(LOG, "1.61"), Some("Faster.\nAnd more.\n\n- one"));
        assert_eq!(entry(LOG, "1.6"), Some("First.\n\n### Detail\n- two"));
        assert_eq!(entry(LOG, "1.618"), None);
        assert_eq!(summary(entry(LOG, "1.61").unwrap()), "Faster. And more.");
    }
}
