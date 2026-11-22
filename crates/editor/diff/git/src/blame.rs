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
/// committed yet. Blamed by words ([`BlameOpts::words`]), `runs` too: each
/// word's, space's and mark's byte range of the text and its commit, in
/// order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    pub commits: Vec<BlameCommit>,
    pub lines: Vec<Option<usize>>,
    pub runs: Vec<(usize, usize, Option<usize>)>,
}

/// How a blame is worked out (the viewer's Blame settings).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlameOpts {
    /// By words (a word changed in a line is its writer's, the rest the
    /// line's earlier writers'), not lines.
    pub words: bool,
    /// Spacing changed alone is no change: the text keeps its writer.
    pub ignore_space: bool,
    /// Text moved within the file keeps its writer (`git blame -M`).
    pub moved: bool,
    /// Commits skipped: what they changed goes to whoever wrote it before
    /// (`git blame --ignore-rev`).
    pub skip: Vec<ObjectId>,
}

/// A text's units for the diff (lines, or words: a run of letters and
/// digits (any byte past ASCII among them), a run of spaces, or one other
/// byte): each one's key and its byte range. Spacing ignored, a line's key
/// is of its text with its spaces dropped, and every run of spaces one key.
fn units(text: &[u8], words: bool, ignore_space: bool) -> (Vec<u64>, Vec<(usize, usize)>) {
    let h = |b: &[u8]| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b.hash(&mut h);
        h.finish()
    };
    let mut keys = Vec::new();
    let mut spans = Vec::new();
    if !words {
        if text.is_empty() {
            return (keys, spans);
        }
        let mut at = 0;
        let body = text.strip_suffix(b"\n").unwrap_or(text);
        for l in body.split(|&c| c == b'\n') {
            let end = at + l.len();
            let l = l.strip_suffix(b"\r").unwrap_or(l);
            keys.push(if ignore_space {
                let squeezed: Vec<u8> = l
                    .iter()
                    .copied()
                    .filter(|c| !c.is_ascii_whitespace())
                    .collect();
                h(&squeezed)
            } else {
                h(l)
            });
            spans.push((at, (end + 1).min(text.len())));
            at = end + 1;
        }
        return (keys, spans);
    }
    let class = |c: u8| {
        if c.is_ascii_alphanumeric() || c >= 0x80 || c == b'_' || c == b'@' || c == b'\\' {
            0
        } else if c.is_ascii_whitespace() {
            1
        } else {
            2
        }
    };
    let mut i = 0;
    while i < text.len() {
        let k = class(text[i]);
        let mut j = i + 1;
        if k != 2 {
            while j < text.len() && class(text[j]) == k {
                j += 1;
            }
        }
        keys.push(if k == 1 && ignore_space {
            h(b" ")
        } else {
            h(&text[i..j])
        });
        spans.push((i, j));
        i = j;
    }
    (keys, spans)
}

/// Whether a unit is spacing only (spacing ignored: never a change of its own).
fn blank(text: &[u8], span: (usize, usize)) -> bool {
    text[span.0..span.1].iter().all(u8::is_ascii_whitespace)
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
    opts: BlameOpts,
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
            self.lines.insert(
                blob,
                units(&data, self.opts.words, self.opts.ignore_space).0,
            );
        }
        Ok(&self.lines[&blob])
    }

    /// Blob `to`'s lines matched in blob `from`: line of `to` → line of `from`.
    fn matched(&mut self, from: ObjectId, to: ObjectId) -> Result<&HashMap<usize, usize>, Error> {
        if !self.matched.contains_key(&(from, to)) {
            let a = self.keys(from)?.clone();
            let b = self.keys(to)?.clone();
            let mut m: HashMap<usize, usize> = phitex_diff::myers::patience(&a, &b)
                .into_iter()
                .map(|(i, j)| (j, i))
                .collect();
            if self.opts.moved {
                moved(&a, &b, &mut m, self.opts.words);
            }
            self.matched.insert((from, to), m);
        }
        Ok(&self.matched[&(from, to)])
    }
}

/// Units of `b` the diff did not match, found as they are among the
/// units of `a` it did not match either (text moved): matched there. By
/// lines, a line of more than spaces; by words, a run of four units or
/// more, the same run in `a`.
fn moved(old: &[u64], new: &[u64], pairs: &mut HashMap<usize, usize>, words: bool) {
    let used: std::collections::HashSet<usize> = pairs.values().copied().collect();
    let mut free: HashMap<u64, Vec<usize>> = HashMap::new();
    for (at, unit) in old.iter().enumerate() {
        if !used.contains(&at) {
            free.entry(*unit).or_default().push(at);
        }
    }
    if !words {
        let blank = {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            b"".as_slice().hash(&mut hasher);
            hasher.finish()
        };
        for (from, unit) in new.iter().enumerate() {
            if pairs.contains_key(&from) || *unit == blank {
                continue;
            }
            if let Some(spots) = free.get_mut(unit)
                && !spots.is_empty()
            {
                pairs.insert(from, spots.remove(0));
            }
        }
        return;
    }
    let mut from = 0;
    let mut taken: std::collections::HashSet<usize> = std::collections::HashSet::new();
    while from < new.len() {
        if pairs.contains_key(&from) {
            from += 1;
            continue;
        }
        let mut to = from;
        while to < new.len() && !pairs.contains_key(&to) {
            to += 1;
        }
        // (the unmatched run new[from..to]: its longest stretch of four or more found whole in old's free units)
        let mut start = from;
        while start + 4 <= to {
            let mut best: Option<(usize, usize)> = None;
            for &at in free.get(&new[start]).map_or(&[][..], Vec::as_slice) {
                let mut len = 0;
                while start + len < to
                    && at + len < old.len()
                    && old[at + len] == new[start + len]
                    && !used.contains(&(at + len))
                    && !taken.contains(&(at + len))
                {
                    len += 1;
                }
                if len >= 4 && best.is_none_or(|(_, bn)| len > bn) {
                    best = Some((at, len));
                }
            }
            match best {
                Some((at, len)) => {
                    for unit in 0..len {
                        pairs.insert(start + unit, at + unit);
                        taken.insert(at + unit);
                    }
                    start += len;
                }
                None => start += 1,
            }
        }
        from = to;
    }
}

/// Where unit `l` of a commit's text, which the diff did not match (it
/// changed it), was in its parent's (`n` units): at the same place within
/// the hunk the commit replaced (the units between the matched ones
/// around it), its last one past that; none where the commit only added.
fn same_place(pairs: &[(usize, usize)], l: usize, n: usize) -> Option<usize> {
    // (`pairs`: the matched units, (child, parent), by child)
    let k = pairs.partition_point(|&(c, _)| c < l);
    let before = k.checked_sub(1).map(|i| pairs[i]);
    let after = pairs.get(k).copied();
    let start = before.map_or(0, |(_, p)| p + 1);
    let end = after.map_or(n, |(_, p)| p);
    if start >= end {
        return None;
    }
    let off = l - before.map_or(0, |(c, _)| c + 1);
    Some((start + off).min(end - 1))
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
        self.blame_with(path, now, &BlameOpts::default())
    }

    /// [`Repo::blame`], worked out as `opts` says: by words or lines,
    /// spacing ignored or counted, moved text kept with its writer, some
    /// commits skipped.
    ///
    /// # Errors
    ///
    /// If the history cannot be read.
    #[allow(clippy::too_many_lines, reason = "one walk, as git blame's")]
    pub fn blame_with(
        &self,
        path: &str,
        now: Option<&[u8]>,
        opts: &BlameOpts,
    ) -> Result<Blame, Error> {
        let full = phitex_diff::flatten::normalize(&format!("{}{path}", self.prefix));
        let mut w = Walk {
            repo: self,
            path: full,
            opts: opts.clone(),
            lines: HashMap::new(),
            commits: HashMap::new(),
            matched: HashMap::new(),
        };
        let head = self.repo.head_id().ok().map(gix::Id::detach);
        let head_blob = match head {
            Some(h) => w.commit(h)?.0,
            None => None,
        };
        let now_text: Vec<u8> = match (now, head_blob) {
            (Some(t), _) => t.to_vec(),
            (None, Some(b)) => self.repo.find_object(b).map_err(git)?.data.clone(),
            (None, None) => Vec::new(),
        };
        let (now_keys, now_spans) = units(&now_text, opts.words, opts.ignore_space);
        let mut out = Blame {
            commits: Vec::new(),
            lines: vec![None; if opts.words { 0 } else { now_keys.len() }],
            runs: Vec::new(),
        };
        let mut unit: Vec<Option<usize>> = vec![None; now_keys.len()];
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
            // (spacing ignored: a unit of spaces alone is never the change, its writer the text's before it)
            let skip = opts.skip.contains(&id);
            for (pi, p) in parents.into_iter().enumerate() {
                if lines.is_empty() {
                    break;
                }
                let (pblob, _, ptime) = w.commit(p)?;
                let Some(pblob) = pblob else { continue };
                let mut passed: Vec<(usize, usize)>;
                if pblob == blob {
                    passed = std::mem::take(&mut lines);
                } else {
                    let m = w.matched(pblob, blob)?.clone();
                    let (to, keep): (Vec<_>, Vec<_>) =
                        lines.iter().partition(|(l, _)| m.contains_key(l));
                    passed = to.into_iter().map(|(l, f)| (m[&l], f)).collect();
                    lines = keep;
                    // (a skipped commit: what it changed passed to its first parent, at the same place in the hunk it changed)
                    if skip && pi == 0 && !lines.is_empty() {
                        let n = w.keys(pblob)?.len();
                        let mut pairs: Vec<(usize, usize)> =
                            m.iter().map(|(&c, &p)| (c, p)).collect();
                        pairs.sort_unstable();
                        let (moved_on, keep): (Vec<_>, Vec<_>) = lines
                            .iter()
                            .map(|&(l, f)| (same_place(&pairs, l, n), l, f))
                            .partition(|(p, _, _)| p.is_some());
                        passed.extend(moved_on.into_iter().map(|(p, _, f)| (p.unwrap_or(0), f)));
                        lines = keep.into_iter().map(|(_, l, f)| (l, f)).collect();
                    }
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
                unit[f] = Some(k);
            }
        }
        if opts.words {
            // (spacing ignored: a run of spaces is its neighbours' writer's, the newer of the two)
            if opts.ignore_space {
                let time = |k: Option<usize>| k.map_or(i64::MAX, |k| out.commits[k].time);
                for i in 0..unit.len() {
                    if blank(&now_text, now_spans[i]) {
                        let a = if i > 0 { unit[i - 1] } else { None };
                        let b = unit.get(i + 1).copied().flatten();
                        let best = match (i > 0, i + 1 < unit.len()) {
                            (true, true) => {
                                if time(a) >= time(b) {
                                    a
                                } else {
                                    b
                                }
                            }
                            (true, false) => a,
                            (false, true) => b,
                            _ => unit[i],
                        };
                        if unit[i].is_some() || best.is_none() {
                            unit[i] = best.or(unit[i]);
                        }
                    }
                }
            }
            for (i, &(a, b)) in now_spans.iter().enumerate() {
                match out.runs.last_mut() {
                    Some(r) if r.2 == unit[i] && r.1 == a => r.1 = b,
                    _ => out.runs.push((a, b, unit[i])),
                }
            }
        } else {
            out.lines = unit;
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
        self.last_change_skipping(path, now, &[])
    }

    /// [`Self::last_change`], the commits in `skip` passed over: a skipped
    /// commit's version of the file goes to whoever made the one before it
    /// (its first parent's), as a skipped commit's lines do.
    pub fn last_change_skipping(
        &self,
        path: &str,
        now: Option<&[u8]>,
        skip: &[ObjectId],
    ) -> Result<Option<BlameCommit>, Error> {
        let full = phitex_diff::flatten::normalize(&format!("{}{path}", self.prefix));
        let mut w = Walk {
            repo: self,
            path: full,
            opts: BlameOpts::default(),
            lines: HashMap::new(),
            commits: HashMap::new(),
            matched: HashMap::new(),
        };
        let Ok(head) = self.repo.head_id() else {
            return Ok(None);
        };
        let mut at = head.detach();
        let Some(mut blob) = w.commit(at)?.0 else {
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
            for &p in &parents {
                if w.commit(p)?.0 == Some(blob) {
                    same = Some(p);
                    break;
                }
            }
            if let Some(p) = same {
                at = p;
                continue;
            }
            // (skipped: on to its first parent's version, if it had one)
            if skip.contains(&at)
                && let Some(&p) = parents.first()
                && let Some(b) = w.commit(p)?.0
            {
                (at, blob) = (p, b);
                continue;
            }
            return self.blame_commit(at).map(Some);
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
    use super::units;

    fn keys(t: &[u8]) -> Vec<u64> {
        units(t, false, false).0
    }

    #[test]
    fn a_last_line_with_no_newline_is_the_same_line() {
        assert_eq!(keys(b"a\nb"), keys(b"a\nb\n"));
        assert_eq!(keys(b"a\r\nb\r\n"), keys(b"a\nb\n"));
        assert_eq!(keys(b"").len(), 0);
        assert_eq!(keys(b"\n").len(), 1);
        assert_eq!(keys(b"a\n\nb\n").len(), 3);
    }

    #[test]
    fn words_are_runs_of_letters_spaces_or_one_mark() {
        let (k, s) = units(b"Hi,  there\\x", true, true);
        assert_eq!(s, [(0, 2), (2, 3), (3, 5), (5, 12)]);
        assert_eq!(k.len(), 4);
        // (spacing ignored: any run of spaces is one key)
        assert_eq!(units(b" ", true, true).0, units(b"   ", true, true).0);
        assert_ne!(units(b" ", true, false).0, units(b"   ", true, false).0);
    }
}
