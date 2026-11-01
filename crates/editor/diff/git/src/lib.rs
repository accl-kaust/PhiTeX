//! A project's past versions, read from its git repository with gitoxide
//! (pure Rust): no checkout, no `git` process, no network. The baselines
//! [`phitex_diff`] diffs the working tree against.
//!
//! - [`Repo::discover`]: the repository a project directory is in (a clear
//!   error outside one).
//! - [`Repo::snapshot`]: a version, by any revision `git rev-parse` takes
//!   (`HEAD`, `HEAD~3`, a branch, a tag, a hash), or [`STAGED`] (the
//!   index): its files, read from the object store as they are asked for
//!   ([`phitex_diff::Files`]; the old version's own `\input` graph is
//!   followed by the flattening that reads them).
//! - [`Repo::refs`] and [`Repo::log`]: what a baseline picker lists, the
//!   commits in topological order with their parents and refs (a graph's).
//! - [`Snapshot::moved`]: whether the revision a snapshot was taken by
//!   names another commit now (a branch that moved; a hash never does).
//!
//! Paths are the project's: relative to the directory given to
//! [`Repo::discover`], which may be below the repository's root.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use gix::ObjectId;

/// The revision that names the index: `staged` against the working tree
/// is what `git diff` shows; `HEAD` against `staged` what `git diff
/// --staged` shows.
pub const STAGED: &str = "staged";

/// Why a repository or a version could not be read.
#[derive(Debug)]
pub enum Error {
    /// The directory is in no git repository.
    NotARepo(PathBuf),
    /// A revision that names no commit, and why.
    BadRev(String, String),
    /// Anything else gitoxide reports.
    Git(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotARepo(d) => write!(
                f,
                "{} is not in a git repository: compare two folders instead",
                d.display()
            ),
            Error::BadRev(r, e) => write!(f, "`{r}` names no commit: {e}"),
            Error::Git(e) => write!(f, "git: {e}"),
        }
    }
}

impl std::error::Error for Error {}

#[allow(clippy::needless_pass_by_value)]
fn git<E: std::fmt::Display>(e: E) -> Error {
    Error::Git(e.to_string())
}

/// A project directory's repository.
pub struct Repo {
    repo: gix::Repository,
    /// The project directory, relative to the repository's root (`""` or
    /// ending in `/`).
    prefix: String,
}

/// A ref, for a picker: its short name, what it is, and its commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefInfo {
    pub name: String,
    pub kind: RefKind,
    pub id: ObjectId,
}

/// What a ref is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Head,
    /// A local branch.
    Branch,
    /// A remote-tracking branch (`origin/main`).
    Remote,
    Tag,
}

/// A commit, for a picker's graph: its id (full, and as short as is
/// unambiguous), its parents in order (the first first), its author's
/// name, its time (seconds since the epoch, and its time zone's offset in
/// seconds), its subject line, and the refs at it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    pub id: ObjectId,
    pub short: String,
    pub parents: Vec<ObjectId>,
    pub author: String,
    pub time: i64,
    pub offset: i32,
    pub subject: String,
    pub refs: Vec<RefInfo>,
}

impl Repo {
    /// The repository project directory `dir` is in.
    ///
    /// # Errors
    ///
    /// [`Error::NotARepo`] if there is none.
    pub fn discover(dir: &Path) -> Result<Repo, Error> {
        let repo = gix::discover(dir).map_err(|_| Error::NotARepo(dir.to_path_buf()))?;
        let root = repo
            .workdir()
            .ok_or_else(|| Error::Git("a bare repository has no project directory".into()))?
            .canonicalize()
            .map_err(git)?;
        let dir = dir.canonicalize().map_err(git)?;
        let rel = dir.strip_prefix(&root).map_err(git)?;
        let mut prefix = rel.to_string_lossy().replace('\\', "/");
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        Ok(Repo { repo, prefix })
    }

    /// The project directory, relative to the repository's root.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The commit `rev` names now.
    ///
    /// # Errors
    ///
    /// If it names none.
    pub fn resolve(&self, rev: &str) -> Result<ObjectId, Error> {
        let bad = |e: &dyn std::fmt::Display| Error::BadRev(rev.to_owned(), e.to_string());
        let id = self.repo.rev_parse_single(rev).map_err(|e| bad(&e))?;
        let commit = id
            .object()
            .map_err(git)?
            .peel_to_commit()
            .map_err(|e| bad(&e))?;
        Ok(commit.id)
    }

    /// The version `rev` names: a commit, or the index for [`STAGED`].
    ///
    /// # Errors
    ///
    /// If `rev` names no commit, or the index cannot be read.
    pub fn snapshot(&self, rev: &str) -> Result<Snapshot, Error> {
        let source = if rev == STAGED {
            let index = self.repo.index_or_empty().map_err(git)?;
            let blobs = index
                .entries()
                .iter()
                .map(|e| (e.path(&index).to_string(), e.id))
                .collect();
            Source::Index(blobs)
        } else {
            let id = self.resolve(rev)?;
            let tree = self
                .repo
                .find_commit(id)
                .map_err(git)?
                .tree_id()
                .map_err(git)?
                .detach();
            Source::Commit { id, tree }
        };
        Ok(Snapshot {
            repo: self.repo.clone(),
            prefix: self.prefix.clone(),
            rev: rev.to_owned(),
            source,
            cache: RefCell::new(HashMap::new()),
        })
    }

    /// `HEAD`, the local branches, the remote-tracking branches and the
    /// tags, each with its commit (an annotated tag's, peeled): `HEAD`
    /// first, then each kind by name.
    ///
    /// # Errors
    ///
    /// If the refs cannot be read.
    pub fn refs(&self) -> Result<Vec<RefInfo>, Error> {
        let mut out = Vec::new();
        if let Ok(id) = self.repo.head_id() {
            out.push(RefInfo {
                name: "HEAD".into(),
                kind: RefKind::Head,
                id: id.detach(),
            });
        }
        let platform = self.repo.references().map_err(git)?;
        for (kind, iter) in [
            (RefKind::Branch, platform.local_branches().map_err(git)?),
            (RefKind::Remote, platform.remote_branches().map_err(git)?),
            (RefKind::Tag, platform.tags().map_err(git)?),
        ] {
            let mut refs = Vec::new();
            for r in iter {
                let Ok(mut r) = r else { continue };
                let name = r.name().shorten().to_string();
                let Ok(id) = r.peel_to_id() else { continue };
                refs.push(RefInfo {
                    name,
                    kind,
                    id: id.detach(),
                });
            }
            refs.sort_by(|a, b| a.name.cmp(&b.name));
            out.extend(refs);
        }
        Ok(out)
    }

    /// The commits reachable from any ref, in topological order (`git log
    /// --all --topo-order`: newest first, no parent before its children,
    /// lines of history not intermixed), `skip` of them skipped and at most
    /// `limit` given: each with its parents in order and the refs at it,
    /// all a commit graph's picker draws.
    ///
    /// # Errors
    ///
    /// If the refs or the history cannot be read.
    pub fn log(&self, skip: usize, limit: usize) -> Result<Vec<CommitInfo>, Error> {
        let refs = self.refs()?;
        let mut at: HashMap<ObjectId, Vec<RefInfo>> = HashMap::new();
        for r in &refs {
            at.entry(r.id).or_default().push(r.clone());
        }
        let mut tips: Vec<ObjectId> = Vec::new();
        for r in &refs {
            // (a tag of a tree or a blob is no commit's)
            if !tips.contains(&r.id) && self.repo.find_commit(r.id).is_ok() {
                tips.push(r.id);
            }
        }
        let walk = gix::traverse::commit::topo::Builder::from_iters(
            &self.repo.objects,
            tips,
            None::<Vec<ObjectId>>,
        )
        .sorting(gix::traverse::commit::topo::Sorting::TopoOrder)
        .build()
        .map_err(git)?;
        let mut out = Vec::new();
        for info in walk.skip(skip).take(limit) {
            let id = info.map_err(git)?.id;
            let commit = self.repo.find_commit(id).map_err(git)?;
            let subject = commit
                .message()
                .map(|m| m.summary().to_string())
                .unwrap_or_default();
            let author = commit
                .author()
                .map(|a| a.name.to_string())
                .unwrap_or_default();
            let time = commit.time().map_err(git)?;
            out.push(CommitInfo {
                id,
                short: commit.id().shorten_or_id().to_string(),
                parents: commit.parent_ids().map(gix::Id::detach).collect(),
                author,
                time: time.seconds,
                offset: time.offset,
                subject,
                refs: at.remove(&id).unwrap_or_default(),
            });
        }
        Ok(out)
    }
}

/// Where a snapshot's files are.
enum Source {
    /// A commit's tree.
    Commit { id: ObjectId, tree: ObjectId },
    /// The index: each path's blob.
    Index(HashMap<String, ObjectId>),
}

/// A version of the project: its files, read from the object store as
/// they are asked for (and kept).
pub struct Snapshot {
    repo: gix::Repository,
    prefix: String,
    rev: String,
    source: Source,
    cache: RefCell<HashMap<String, Option<String>>>,
}

impl Snapshot {
    /// The revision it was taken by.
    #[must_use]
    pub fn rev(&self) -> &str {
        &self.rev
    }

    /// Its commit (`None`: the index).
    #[must_use]
    pub fn commit(&self) -> Option<ObjectId> {
        match self.source {
            Source::Commit { id, .. } => Some(id),
            Source::Index(_) => None,
        }
    }

    /// Whether its revision names another commit now (a branch that moved,
    /// a new `HEAD`): that commit, to take a new snapshot by. A hash never
    /// moves; the index is read afresh by a new snapshot.
    ///
    /// # Errors
    ///
    /// If the revision names no commit any more.
    pub fn moved(&self, repo: &Repo) -> Result<Option<ObjectId>, Error> {
        match self.source {
            Source::Commit { id, .. } => {
                let now = repo.resolve(&self.rev)?;
                Ok((now != id).then_some(now))
            }
            Source::Index(_) => Ok(None),
        }
    }

    /// The bytes of the project's file `path`.
    fn blob(&self, path: &str) -> Option<Vec<u8>> {
        let full = phitex_diff::flatten::normalize(&format!("{}{path}", self.prefix));
        let id = match &self.source {
            Source::Commit { tree, .. } => {
                let tree = self.repo.find_tree(*tree).ok()?;
                let entry = tree.lookup_entry_by_path(&full).ok()??;
                if !entry.mode().is_blob() {
                    return None;
                }
                entry.object_id()
            }
            Source::Index(blobs) => *blobs.get(&full)?,
        };
        Some(self.repo.find_object(id).ok()?.data.clone())
    }
}

impl phitex_diff::Files for Snapshot {
    fn read(&self, path: &str) -> Option<String> {
        if let Some(t) = self.cache.borrow().get(path) {
            return t.clone();
        }
        let text = self
            .blob(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        self.cache
            .borrow_mut()
            .insert(path.to_owned(), text.clone());
        text
    }
}
