//! Who last changed each line of a file, as `git blame` says (its lines
//! passed from each commit to its parents, newest commit first, through a
//! line diff, [`phitex_diff::myers::patience`]): the viewer's Blame, an
//! overlay on the pages. A line the working tree (or the editor's buffer)
//! has and `HEAD` does not is not committed yet. Renames are not followed:
//! a file's lines older than its rename are the rename's.

use std::collections::{BinaryHeap, HashMap};
use std::hash::{Hash, Hasher};

use gix::ObjectId;

use crate::{Error, Repo, git};

/// A commit a blame names: its id (full and short), its author's name
/// and email, the author's time (seconds since the epoch), its subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameCommit {
    pub id: ObjectId,
    pub short: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub subject: String,
}

/// A file's blame: the commits it names, and each line's (0-based, as the
/// text blamed has them), an index into `commits`, or `None`: not
/// committed yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    pub commits: Vec<BlameCommit>,
    pub lines: Vec<Option<usize>>,
}

/// A text's lines as keys for the diff: each line's hash, its end of line
/// (`\n`, `\r\n`) left out, so a last line with no newline matches one with.
fn keys(text: &[u8]) -> Vec<u64> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix(b"\n").unwrap_or(text);
    body.split(|&c| c == b'\n')
        .map(|l| {
            let l = l.strip_suffix(b"\r").unwrap_or(l);
            let mut h = std::collections::hash_map::DefaultHasher::new();
            l.hash(&mut h);
            h.finish()
        })
        .collect()
}

/// A commit with lines still to place, in the walk's queue: newest
/// (committer time) first.
#[derive(PartialEq, Eq)]
struct Queued {
    time: i64,
    id: ObjectId,
}

impl Ord for Queued {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.time.cmp(&o.time).then(self.id.cmp(&o.id))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

/// What the walk keeps: each blob's line keys, each commit's blob of the
/// file and its parents and time, each pair of blobs' matched lines.
struct Walk<'r> {
    repo: &'r Repo,
    path: String,
    lines: HashMap<ObjectId, Vec<u64>>,
    commits: HashMap<ObjectId, (Option<ObjectId>, Vec<ObjectId>, i64)>,
    matched: HashMap<(ObjectId, ObjectId), HashMap<usize, usize>>,
}

impl Walk<'_> {
    /// Commit `id`'s blob of the file (none: no such file there), its
    /// parents and its committer time.
    fn commit(&mut self, id: ObjectId) -> Result<(Option<ObjectId>, Vec<ObjectId>, i64), Error> {
        if let Some(c) = self.commits.get(&id) {
            return Ok(c.clone());
        }
        let c = self.repo.repo.find_commit(id).map_err(git)?;
        let tree = self
            .repo
            .repo
            .find_tree(c.tree_id().map_err(git)?)
            .map_err(git)?;
        let blob = tree
            .lookup_entry_by_path(&self.path)
            .map_err(git)?
            .filter(|e| e.mode().is_blob())
            .map(|e| e.object_id());
        let parents = c.parent_ids().map(gix::Id::detach).collect();
        let time = c.time().map_err(git)?.seconds;
        let out = (blob, parents, time);
        self.commits.insert(id, out.clone());
        Ok(out)
    }

    fn keys(&mut self, blob: ObjectId) -> Result<&Vec<u64>, Error> {
        if !self.lines.contains_key(&blob) {
            let data = self.repo.repo.find_object(blob).map_err(git)?.data.clone();
            self.lines.insert(blob, keys(&data));
        }
        Ok(&self.lines[&blob])
    }

    /// Blob `to`'s lines matched in blob `from`: line of `to` → line of `from`.
    fn matched(&mut self, from: ObjectId, to: ObjectId) -> Result<&HashMap<usize, usize>, Error> {
        if !self.matched.contains_key(&(from, to)) {
            let a = self.keys(from)?.clone();
            let b = self.keys(to)?;
            let m = phitex_diff::myers::patience(&a, b)
                .into_iter()
                .map(|(i, j)| (j, i))
                .collect();
            self.matched.insert((from, to), m);
        }
        Ok(&self.matched[&(from, to)])
    }
}

impl Repo {
    /// The project's file `path` blamed: each line of `now` (the file as
    /// the build reads it: the working tree's, an editor's buffer; `None`:
    /// as `HEAD` has it) with the commit that last changed it, or none
    /// where `HEAD` does not have it (not committed yet).
    ///
    /// # Errors
    ///
    /// If the history cannot be read.
    pub fn blame(&self, path: &str, now: Option<&[u8]>) -> Result<Blame, Error> {
        let full = phitex_diff::flatten::normalize(&format!("{}{path}", self.prefix));
        let mut w = Walk {
            repo: self,
            path: full,
            lines: HashMap::new(),
            commits: HashMap::new(),
            matched: HashMap::new(),
        };
        let head = self.repo.head_id().ok().map(gix::Id::detach);
        let head_blob = match head {
            Some(h) => w.commit(h)?.0,
            None => None,
        };
        let now_keys = match (now, head_blob) {
            (Some(t), _) => keys(t),
            (None, Some(b)) => w.keys(b)?.clone(),
            (None, None) => Vec::new(),
        };
        let mut out = Blame {
            commits: Vec::new(),
            lines: vec![None; now_keys.len()],
        };
        let (Some(head), Some(head_blob)) = (head, head_blob) else {
            return Ok(out);
        };
        // (the lines `HEAD` has: (its line, the line of `now`))
        let at_head: Vec<(usize, usize)> = {
            let h = w.keys(head_blob)?.clone();
            phitex_diff::myers::patience(&h, &now_keys)
        };
        let mut pending: HashMap<ObjectId, Vec<(usize, usize)>> = HashMap::new();
        let mut queue = BinaryHeap::new();
        if !at_head.is_empty() {
            let time = w.commit(head)?.2;
            pending.insert(head, at_head);
            queue.push(Queued { time, id: head });
        }
        let mut index: HashMap<ObjectId, usize> = HashMap::new();
        while let Some(Queued { id, .. }) = queue.pop() {
            let Some(mut lines) = pending.remove(&id) else {
                continue;
            };
            let (blob, parents, _) = w.commit(id)?;
            let Some(blob) = blob else { continue };
            for p in parents {
                if lines.is_empty() {
                    break;
                }
                let (pblob, _, ptime) = w.commit(p)?;
                let Some(pblob) = pblob else { continue };
                let passed: Vec<(usize, usize)>;
                if pblob == blob {
                    passed = std::mem::take(&mut lines);
                } else {
                    let m = w.matched(pblob, blob)?;
                    let (to, keep): (Vec<_>, Vec<_>) =
                        lines.iter().partition(|(l, _)| m.contains_key(l));
                    passed = to.into_iter().map(|(l, f)| (m[&l], f)).collect();
                    lines = keep;
                }
                if passed.is_empty() {
                    continue;
                }
                let e = pending.entry(p).or_default();
                if e.is_empty() {
                    queue.push(Queued { time: ptime, id: p });
                }
                e.extend(passed);
            }
            if lines.is_empty() {
                continue;
            }
            let k = if let Some(&k) = index.get(&id) {
                k
            } else {
                out.commits.push(self.blame_commit(id)?);
                index.insert(id, out.commits.len() - 1);
                out.commits.len() - 1
            };
            for (_, f) in lines {
                out.lines[f] = Some(k);
            }
        }
        Ok(out)
    }

    /// The commit that last changed the project's file `path` (a figure's
    /// image, blamed whole), from `HEAD`: none where `HEAD` has no such
    /// file, or where `now` (its bytes as they are) differs from `HEAD`'s
    /// (not committed yet).
    ///
    /// # Errors
    ///
    /// If the history cannot be read.
    pub fn last_change(
        &self,
        path: &str,
        now: Option<&[u8]>,
    ) -> Result<Option<BlameCommit>, Error> {
        let full = phitex_diff::flatten::normalize(&format!("{}{path}", self.prefix));
        let mut w = Walk {
            repo: self,
            path: full,
            lines: HashMap::new(),
            commits: HashMap::new(),
            matched: HashMap::new(),
        };
        let Ok(head) = self.repo.head_id() else {
            return Ok(None);
        };
        let mut at = head.detach();
        let Some(blob) = w.commit(at)?.0 else {
            return Ok(None);
        };
        if let Some(now) = now
            && self.repo.find_object(blob).map_err(git)?.data != now
        {
            return Ok(None);
        }
        // (back while a parent has the same bytes: the first such parent, as the blame's lines go)
        loop {
            let (_, parents, _) = w.commit(at)?;
            let mut same = None;
            for p in parents {
                if w.commit(p)?.0 == Some(blob) {
                    same = Some(p);
                    break;
                }
            }
            match same {
                Some(p) => at = p,
                None => return self.blame_commit(at).map(Some),
            }
        }
    }

    fn blame_commit(&self, id: ObjectId) -> Result<BlameCommit, Error> {
        let c = self.repo.find_commit(id).map_err(git)?;
        let subject = c
            .message()
            .map(|m| m.summary().to_string())
            .unwrap_or_default();
        let a = c.author().map_err(git)?;
        let time = a
            .time()
            .map(|t| t.seconds)
            .or_else(|_| c.time().map(|t| t.seconds))
            .map_err(git)?;
        Ok(BlameCommit {
            id,
            short: c.id().shorten_or_id().to_string(),
            author: a.name.to_string(),
            email: a.email.to_string(),
            time,
            subject,
        })
    }

    /// A commit's time (the committer's, seconds since the epoch): a ref's,
    /// for "changed since" a tag or a branch.
    ///
    /// # Errors
    ///
    /// If it is no commit.
    pub fn commit_time(&self, id: ObjectId) -> Result<i64, Error> {
        Ok(self
            .repo
            .find_commit(id)
            .map_err(git)?
            .time()
            .map_err(git)?
            .seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::keys;

    #[test]
    fn a_last_line_with_no_newline_is_the_same_line() {
        assert_eq!(keys(b"a\nb"), keys(b"a\nb\n"));
        assert_eq!(keys(b"a\r\nb\r\n"), keys(b"a\nb\n"));
        assert_eq!(keys(b"").len(), 0);
        assert_eq!(keys(b"\n").len(), 1);
        assert_eq!(keys(b"a\n\nb\n").len(), 3);
    }
}
