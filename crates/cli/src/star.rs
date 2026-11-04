//! Once, after ten different documents built without errors: where to
//! star PhiTeX, counted on this machine only (no network), said once and
//! never again (`$XDG_STATE_HOME/phitex/star`, else
//! `~/.local/state/phitex/star`: the documents' hashes until the tenth,
//! then `said`). Never in `--json`, `--quiet`, `--lsp`, under CI or
//! `PHITEX_NO_NEWS`, or without a terminal (an editor's run).
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use crate::ui::Ui;

/// The project's page.
pub const REPO: &str = "https://github.com/accl-kaust/PhiTeX";

/// The documents built before it is said.
const AFTER: usize = 10;

fn state_file() -> Option<PathBuf> {
    let env = |k| std::env::var_os(k).filter(|d| !d.is_empty());
    let d = env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))?;
    Some(d.join("phitex").join("star"))
}

/// The state's text with document `id` counted: the new text, and
/// whether this is the time to say it. `said` stays said.
fn counted(old: &str, id: &str) -> (String, bool) {
    if old.lines().any(|l| l == "said") {
        return (old.to_owned(), false);
    }
    let mut ids: Vec<&str> = old.lines().filter(|l| !l.is_empty()).collect();
    if !ids.contains(&id) {
        ids.push(id);
    }
    if ids.len() >= AFTER {
        ("said\n".to_owned(), true)
    } else {
        (ids.join("\n") + "\n", false)
    }
}

/// Document `main` built without errors: counted, and on the tenth the
/// line said (once).
pub fn built(main: &Path, ui: &Ui, quiet: bool) {
    if quiet
        || ui.json
        || ui.quiet
        || !std::io::stderr().is_terminal()
        || std::env::var_os("CI").is_some()
        || std::env::var_os("PHITEX_NO_NEWS").is_some()
    {
        return;
    }
    let Some(f) = state_file() else { return };
    let p = main.canonicalize().unwrap_or_else(|_| main.to_path_buf());
    let id = format!(
        "{:016x}",
        crate::tex::Fnv::new().add(p.to_string_lossy().as_bytes()).0
    );
    let old = std::fs::read_to_string(&f).unwrap_or_default();
    let (new, say) = counted(&old, &id);
    if new == old {
        return;
    }
    // (said only once it is kept as said: an unwritable folder would say it every run)
    let kept = f
        .parent()
        .is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(&f, new).is_ok();
    if kept && say {
        ui.line(&ui.st.dim(&format!(
            "PhiTeX has built {AFTER} of your documents. If it helps you, a star helps others find it: {REPO} (said once)"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn said_on_the_tenth_document_once() {
        let mut s = String::new();
        for k in 0..9 {
            let (n, say) = counted(&s, &format!("d{k}"));
            assert!(!say);
            s = n;
        }
        // (the same document again counts once)
        let (n, say) = counted(&s, "d3");
        assert!(!say);
        s = n;
        let (n, say) = counted(&s, "d9");
        assert!(say);
        assert_eq!(n, "said\n");
        assert_eq!(counted(&n, "d10"), (n.clone(), false));
    }
}
