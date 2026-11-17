//! Blame: who last changed what the pages show, from the project's git
//! history ([`phitex_git::Repo::blame`]), drawn over the pages by the
//! viewer (viewer/src/blame.ts: bars in the margin, a card on hover) and
//! said by `phitex session blame`. An overlay only: no build, no new TeX
//! run, the PDF as it is.
//!
//! - [`files`]: the viewer's `blame` op: the files named, each blamed as
//!   the build reads it (an attached editor's buffer, else the disk's):
//!   a text file's commit for each of its lines, with its text (the
//!   viewer maps the pages' glyph origins, bytes of it, to its lines); a
//!   binary file (a figure's image) its last change; plus the tags and
//!   branches, for "changed since".
//! - [`pages`]: `phitex session blame`'s: each printed line of the pages
//!   (the glyphs on one baseline, a column apart split) with the newest
//!   commit of the source lines it came from.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

use phitex_git::{Blame, BlameCommit, Repo};
use serde_json::{Value, json};

/// A file blamed before: its bytes' hash and `HEAD` then, and its blame
/// (a commit's file blamed again only once either moves).
type Kept = HashMap<String, (u64, String, Blame)>;

fn kept() -> &'static Mutex<Kept> {
    static K: OnceLock<Mutex<Kept>> = OnceLock::new();
    K.get_or_init(Mutex::default)
}

fn hash(b: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

/// Whether `name` is a project file's name (no absolute path, no `..`).
fn safe(name: &str) -> bool {
    !name.starts_with('/') && !name.split('/').any(|c| c == "..") && !name.is_empty()
}

/// Whether a file is blamed whole (an image), not by its lines.
fn binary(name: &str, bytes: &[u8]) -> bool {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "pdf"
            | "eps"
            | "ps"
            | "gif"
            | "bmp"
            | "tif"
            | "tiff"
            | "webp"
            | "jbig2"
            | "jb2"
    ) || bytes.iter().take(8192).any(|&c| c == 0)
}

/// File `name` of the project at `root` as the build reads it: an
/// attached editor's buffer, else the disk's bytes.
fn now(root: &Path, name: &str) -> Option<Vec<u8>> {
    let name = name.strip_prefix("./").unwrap_or(name);
    if let Some(b) = crate::review::hub().buffer(name) {
        return Some(b.into_bytes());
    }
    std::fs::read(root.join(name)).ok()
}

/// The commits named, each once: their JSON, by id.
#[derive(Default)]
struct Commits {
    out: Vec<Value>,
    at: HashMap<phitex_git::ObjectId, usize>,
}

impl Commits {
    fn index(&mut self, c: &BlameCommit) -> usize {
        if let Some(&k) = self.at.get(&c.id) {
            return k;
        }
        self.out.push(json!({
            "id": c.id.to_string(),
            "short": c.short,
            "author": c.author,
            "email": c.email,
            "time": c.time,
            "subject": c.subject,
        }));
        self.at.insert(c.id, self.out.len() - 1);
        self.out.len() - 1
    }
}

/// File `name`'s blame as it is now (`bytes`), kept while its bytes and
/// `HEAD` stay.
fn blame_of(repo: &Repo, head: &str, name: &str, bytes: &[u8]) -> Result<Blame, String> {
    let h = hash(bytes);
    if let Some((kh, khead, b)) = kept()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
        && *kh == h
        && khead == head
    {
        return Ok(b.clone());
    }
    let b = repo.blame(name, Some(bytes)).map_err(|e| e.to_string())?;
    kept()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.to_owned(), (h, head.to_owned(), b.clone()));
    Ok(b)
}

/// The viewer's `blame` op: `files` (the project's, by their names from
/// `root`) blamed. `{commits: [{id, short, author, email, time,
/// subject}], files: {NAME: {text, lines: [commit or -1]} | {last: commit
/// or -1} | {error}}, refs: [{name, kind, time}], now}`: -1 is not
/// committed yet.
///
/// # Errors
///
/// Outside a git repository.
pub fn files(root: &Path, files: &[String]) -> Result<Value, String> {
    let repo = Repo::discover(root).map_err(|e| format!("no git history here ({e})"))?;
    let head = repo
        .resolve("HEAD")
        .map(|h| h.to_string())
        .unwrap_or_default();
    let mut commits = Commits::default();
    let mut out = serde_json::Map::new();
    for name in files {
        if !safe(name) || out.contains_key(name) {
            continue;
        }
        let Some(bytes) = now(root, name) else {
            out.insert(name.clone(), json!({"error": "no such file"}));
            continue;
        };
        if binary(name, &bytes) {
            let last = repo
                .last_change(name, Some(&bytes))
                .map_err(|e| e.to_string())?;
            let k = last.as_ref().map_or(-1, |c| idx(commits.index(c)));
            out.insert(name.clone(), json!({"last": k}));
            continue;
        }
        let b = blame_of(&repo, &head, name, &bytes)?;
        let lines: Vec<i64> = b
            .lines
            .iter()
            .map(|l| l.map_or(-1, |k| idx(commits.index(&b.commits[k]))))
            .collect();
        out.insert(
            name.clone(),
            json!({"text": String::from_utf8_lossy(&bytes), "lines": lines}),
        );
    }
    let refs: Vec<Value> = repo
        .refs()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|r| {
            matches!(
                r.kind,
                phitex_git::RefKind::Tag | phitex_git::RefKind::Branch
            )
        })
        .filter_map(|r| {
            let time = repo.commit_time(r.id).ok()?;
            let kind = if r.kind == phitex_git::RefKind::Tag {
                "tag"
            } else {
                "branch"
            };
            Some(json!({"name": r.name, "kind": kind, "time": time, "id": r.id.to_string()}))
        })
        .collect();
    Ok(json!({
        "commits": commits.out,
        "files": out,
        "refs": refs,
        "now": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    }))
}

fn idx(k: usize) -> i64 {
    i64::try_from(k).unwrap_or(i64::MAX)
}

/// A printed line: its glyphs' indices, on one baseline, none a column
/// apart from the one before.
struct Printed {
    y: f32,
    x0: f32,
    x1: f32,
    glyphs: Vec<usize>,
}

/// A page's glyphs as printed lines: those within a point of one baseline
/// (a rounded baseline split a line whose glyphs straddled the rounding),
/// left to right, a new line where the next glyph is 18 pt or more to the
/// right (the next column).
fn printed(x: &[f32], y: &[f32]) -> Vec<Printed> {
    let mut order: Vec<usize> = (0..x.len()).collect();
    order.sort_by(|&a, &b| y[a].total_cmp(&y[b]));
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for i in order {
        match rows.last_mut() {
            Some(r) if y[i] - y[r[0]] < 1.0 => r.push(i),
            _ => rows.push(vec![i]),
        }
    }
    let mut out: Vec<Printed> = Vec::new();
    for mut r in rows {
        r.sort_by(|&a, &b| x[a].total_cmp(&x[b]));
        let mut at = out.len();
        for i in r {
            match out.get_mut(at) {
                Some(l) if x[i] - l.x1 < 18.0 => {
                    l.x1 = x[i];
                    l.glyphs.push(i);
                }
                _ => {
                    out.push(Printed {
                        y: y[i],
                        x0: x[i],
                        x1: x[i],
                        glyphs: vec![i],
                    });
                    at = out.len() - 1;
                }
            }
        }
    }
    out
}

/// The byte offsets lines start at in `text` (the first, 0).
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            text.bytes()
                .enumerate()
                .filter(|&(_, c)| c == b'\n')
                .map(|(i, _)| i + 1),
        )
        .collect()
}

/// `phitex session blame`'s answer: pages `pages` (their glyph origins:
/// `[{page, file, start, end, x, y}]`, `files` their file names) of the
/// project at `root`, each printed line with the newest commit of the
/// source lines it came from (`uncommitted` where one of them is not
/// committed), and its source (`file`, `lines` [first, last] from 1).
///
/// # Errors
///
/// Outside a git repository.
pub fn pages(root: &Path, names: &[String], pages: &[Value]) -> Result<Value, String> {
    let wanted: Vec<String> = names
        .iter()
        .map(|n| n.strip_prefix("./").unwrap_or(n).to_owned())
        .filter(|n| safe(n) && root.join(n).is_file())
        .collect();
    let b = files(root, &wanted)?;
    let commits = b["commits"].as_array().cloned().unwrap_or_default();
    let time = |k: i64| -> i64 {
        usize::try_from(k)
            .ok()
            .and_then(|k| commits.get(k))
            .and_then(|c| c["time"].as_i64())
            .unwrap_or(i64::MIN)
    };
    // (each file's line starts and its lines' commits, by its index in `names`)
    let per: Vec<Option<(Vec<usize>, Vec<i64>)>> = names
        .iter()
        .map(|n| {
            let f = &b["files"][n.strip_prefix("./").unwrap_or(n)];
            let text = f["text"].as_str()?;
            let lines = f["lines"]
                .as_array()?
                .iter()
                .map(|v| v.as_i64().unwrap_or(-1))
                .collect();
            Some((line_starts(text), lines))
        })
        .collect();
    let f32s = |v: &Value| -> Vec<f32> {
        #[allow(clippy::cast_possible_truncation, reason = "points on a page")]
        v.as_array()
            .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
            .unwrap_or_default()
    };
    let u64s = |v: &Value| -> Vec<u64> {
        v.as_array()
            .map(|a| a.iter().map(|x| x.as_u64().unwrap_or(u64::MAX)).collect())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for p in pages {
        let (x, y) = (f32s(&p["x"]), f32s(&p["y"]));
        let (file, start) = (u64s(&p["file"]), u64s(&p["start"]));
        let mut lines = Vec::new();
        for l in printed(&x, &y) {
            // (the line's source: its glyphs' files and lines, the newest commit of them)
            let mut src: Option<(usize, usize, usize)> = None;
            let mut best: Option<i64> = None;
            let mut uncommitted = false;
            for &g in &l.glyphs {
                let Some(f) = file.get(g).and_then(|&f| usize::try_from(f).ok()) else {
                    continue;
                };
                let Some(Some((starts, commit))) = per.get(f) else {
                    continue;
                };
                let at = usize::try_from(start[g]).unwrap_or(usize::MAX);
                let line = starts.partition_point(|&s| s <= at).saturating_sub(1);
                let c = commit.get(line).copied().unwrap_or(-1);
                if c < 0 {
                    uncommitted = true;
                } else if best.is_none_or(|b| time(c) > time(b)) {
                    best = Some(c);
                }
                src = Some(match src {
                    Some((sf, a, z)) if sf == f => (f, a.min(line), z.max(line)),
                    Some(s) => s,
                    None => (f, line, line),
                });
            }
            let Some((f, a, z)) = src else { continue };
            lines.push(json!({
                "y": (f64::from(l.y) * 10.0).round() / 10.0,
                "x": [(f64::from(l.x0) * 10.0).round() / 10.0, (f64::from(l.x1) * 10.0).round() / 10.0],
                "file": names[f].strip_prefix("./").unwrap_or(&names[f]),
                "lines": [a + 1, z + 1],
                "commit": best,
                "uncommitted": uncommitted,
            }));
        }
        out.push(json!({"page": p["page"], "lines": lines}));
    }
    Ok(json!({"commits": commits, "pages": out}))
}

#[cfg(test)]
mod tests {
    use super::{line_starts, printed};

    #[test]
    fn printed_lines_split_at_baselines_and_columns() {
        // (two columns, two lines each: the right column's glyphs 150 pt over)
        let x = [10.0, 15.0, 20.0, 160.0, 165.0, 10.0, 15.0, 160.0];
        let y = [100.0, 100.0, 100.2, 100.0, 100.0, 112.0, 112.0, 112.0];
        let ls = printed(&x, &y);
        let gs: Vec<Vec<usize>> = ls.iter().map(|l| l.glyphs.clone()).collect();
        assert_eq!(gs, [vec![0, 1, 2], vec![3, 4], vec![5, 6], vec![7]]);
        // (glyphs either side of a half point: one line, not two interleaved)
        let two = printed(&[10.0, 15.0, 20.0, 25.0], &[189.74, 189.76, 189.74, 189.76]);
        assert_eq!(two.len(), 1);
        assert_eq!(two[0].glyphs, [0, 1, 2, 3]);
    }

    #[test]
    fn lines_start_after_each_newline() {
        assert_eq!(line_starts("a\nbc\n"), [0, 2, 5]);
        assert_eq!(line_starts(""), [0]);
    }
}
