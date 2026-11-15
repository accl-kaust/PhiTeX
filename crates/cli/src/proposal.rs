//! Proposals (docs/spec-editor-protocol.md §9.6): the agent's change kept
//! apart from the user's files until the user accepts it, whole or hunk
//! by hunk. A proposal is a set of files, each with the text the agent
//! started from (`base`) and the text it proposes (`new`); its hunks are
//! the word-level hunks between the two. A hunk is applied to the file as
//! it is now by finding its base range there: where nothing changed it
//! since the agent read it, it moves with the edits around it; where the
//! user changed it, the hunk conflicts and is not applied.
//!
//! The workspace (`.phitex/work/`) is where the agent writes a proposal
//! without touching the user's files: a copy of the project's text files,
//! and beside it (`.phitex/work-base/`) the copy as made, the base its
//! proposal is read against.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::review_text as rt;

/// One file of a proposal: its text when the agent read it (none: a file
/// it adds) and the text proposed (none: a file it deletes).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PropFile {
    pub file: String,
    pub base: Option<String>,
    pub new: Option<String>,
}

/// Where byte range `r` of `base` is in `cur`, when no change from `base`
/// to `cur` touches it (changes ending at its start or beginning at its
/// end are around it); none: the user changed it since (a conflict).
#[must_use]
pub fn place(base: &str, cur: &str, r: (usize, usize)) -> Option<(usize, usize)> {
    if base == cur {
        return Some(r);
    }
    let mut delta: isize = 0;
    for c in rt::changes(base, cur) {
        let before = c.a.1 <= r.0 && !(r.0 == r.1 && c.a.0 == r.0 && c.a.1 > c.a.0);
        if before {
            delta += rt::growth(c.a, c.b);
            continue;
        }
        if c.a.0 >= r.1 && !(c.a.0 == r.0 && c.a.1 > c.a.0) {
            break;
        }
        return None;
    }
    let s = r.0.checked_add_signed(delta)?;
    let e = r.1.checked_add_signed(delta)?;
    (e <= cur.len() && cur.is_char_boundary(s) && cur.is_char_boundary(e)).then_some((s, e))
}

/// `cur` with the hunks (their base ranges, old and new text) applied
/// where each is found; the ranges of `base` they replace. Err: the base
/// ranges of the hunks that conflict.
pub fn apply(
    base: &str,
    cur: &str,
    hunks: &[((usize, usize), &str, &str)],
) -> Result<String, Vec<(usize, usize)>> {
    let mut at: Vec<((usize, usize), &str)> = Vec::new();
    let mut bad = Vec::new();
    for &(r, old, new) in hunks {
        match place(base, cur, r) {
            Some(p) if &cur[p.0..p.1] == old => at.push((p, new)),
            _ => bad.push(r),
        }
    }
    if !bad.is_empty() {
        return Err(bad);
    }
    at.sort_by_key(|(p, _)| std::cmp::Reverse(p.0));
    let mut out = cur.to_owned();
    for (p, new) in at {
        out.replace_range(p.0..p.1, new);
    }
    Ok(out)
}

/// The proposals' folder, `.phitex/proposals/`.
#[must_use]
pub fn folder(root: &Path) -> PathBuf {
    root.join(".phitex").join("proposals")
}

/// Proposal `id`'s revision `rev`'s files, as written.
pub fn load(root: &Path, id: &str, rev: u32) -> Result<Vec<PropFile>, String> {
    let p = folder(root).join(id).join(format!("r{rev}.json"));
    let b = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display()))
}

/// Proposal `id`'s revision `rev`'s files written (whole: beside, then
/// renamed).
pub fn store(root: &Path, id: &str, rev: u32, files: &[PropFile]) -> Result<(), String> {
    let d = folder(root).join(id);
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    let p = d.join(format!("r{rev}.json"));
    let tmp = p.with_extension("json.tmp");
    let text = serde_json::to_string(files).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
}

/// The workspace, `.phitex/work/`, and its base, `.phitex/work-base/`.
#[must_use]
pub fn workspace(root: &Path) -> (PathBuf, PathBuf) {
    let d = root.join(".phitex");
    (d.join("work"), d.join("work-base"))
}

/// The workspace made again from `files` (name, text now): both folders
/// emptied and filled with them.
pub fn reset_workspace(root: &Path, files: &[(String, String)]) -> Result<PathBuf, String> {
    let (work, base) = workspace(root);
    for d in [&work, &base] {
        if d.exists() {
            std::fs::remove_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        for (name, text) in files {
            let p = d.join(name);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    Ok(work)
}

/// The text files under `d`, by their names relative to it (`/`-joined).
fn files_under(d: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![d.to_path_buf()];
    while let Some(x) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&x) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(d) {
                let name = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// What the agent changed in the workspace: each file whose text differs
/// from its base (added and deleted files too), as a proposal's files.
#[must_use]
pub fn workspace_changes(root: &Path, reviewed: &dyn Fn(&str) -> bool) -> Vec<PropFile> {
    let (work, base) = workspace(root);
    if !work.is_dir() {
        return Vec::new();
    }
    let read = |p: PathBuf| {
        std::fs::read(p)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let mut names = files_under(&work);
    names.extend(files_under(&base));
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|n| reviewed(n))
        .filter_map(|n| {
            let (b, w) = (read(base.join(&n)), read(work.join(&n)));
            (b != w).then_some(PropFile {
                file: n,
                base: b,
                new: w,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hunk_follows_the_users_edits_around_it_and_conflicts_inside() {
        let base = "The quick brown fox jumps over the lazy dog.";
        let r = (10, 15); // "brown"
        // (an edit before it: moved)
        let cur = "A very quick brown fox jumps over the lazy dog.";
        let p = place(base, cur, r).expect("placed");
        assert_eq!(&cur[p.0..p.1], "brown");
        // (an edit after it: still there)
        let cur = "The quick brown fox jumps over the sleepy dog.";
        assert_eq!(place(base, cur, r), Some(r));
        // (the word itself changed: a conflict)
        let cur = "The quick red fox jumps over the lazy dog.";
        assert_eq!(place(base, cur, r), None);
        // (applied: two hunks, the user's edit kept)
        let cur = "A very quick brown fox jumps over the lazy dog.";
        let out = apply(
            base,
            cur,
            &[((10, 15), "brown", "red"), ((35, 39), "lazy", "idle")],
        )
        .expect("applied");
        assert_eq!(out, "A very quick red fox jumps over the idle dog.");
        let bad = apply(
            base,
            "The quick red fox jumps over the lazy dog.",
            &[((10, 15), "brown", "grey")],
        );
        assert_eq!(bad, Err(vec![(10, 15)]));
        // (an insertion where the user inserted too: still applied after it)
        let out = apply(base, cur, &[((44, 44), "", " Then it sleeps.")]).expect("applied");
        assert_eq!(
            out,
            "A very quick brown fox jumps over the lazy dog. Then it sleeps."
        );
    }

    #[test]
    fn the_workspace_says_what_the_agent_changed() {
        let root = std::env::temp_dir().join(format!("phitex-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a folder");
        let files = vec![
            ("main.tex".to_owned(), "Hello world.\n".to_owned()),
            ("ch/one.tex".to_owned(), "One.\n".to_owned()),
        ];
        let work = reset_workspace(&root, &files).expect("made");
        assert_eq!(workspace_changes(&root, &|_| true), []);
        std::fs::write(work.join("main.tex"), "Hello there.\n").expect("edited");
        std::fs::write(work.join("ch/two.tex"), "Two.\n").expect("added");
        std::fs::remove_file(work.join("ch/one.tex")).expect("deleted");
        let got = workspace_changes(&root, &|_| true);
        assert_eq!(
            got,
            [
                PropFile {
                    file: "ch/one.tex".into(),
                    base: Some("One.\n".into()),
                    new: None
                },
                PropFile {
                    file: "ch/two.tex".into(),
                    base: None,
                    new: Some("Two.\n".into())
                },
                PropFile {
                    file: "main.tex".into(),
                    base: Some("Hello world.\n".into()),
                    new: Some("Hello there.\n".into())
                },
            ]
        );
        store(&root, "p1", 1, &got).expect("stored");
        assert_eq!(load(&root, "p1", 1).expect("loaded"), got);
        let _ = std::fs::remove_dir_all(&root);
    }
}
