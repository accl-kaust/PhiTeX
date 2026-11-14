//! Reviewing with an agent (docs/spec-editor-protocol.md §9): what the
//! running watch knows for `phitex session` and the viewer's notes and
//! changes, in one place shared by the watch's loop, its language
//! server's threads (the session's requests, the editors' edits) and the
//! viewer's requests.
//!
//! - **Builds** ([`Hub::built`]): each build's record, the hashes of the
//!   files it read ([`fed`], from the host's finder), its problems; the
//!   diagnostics made later ([`Hub::diagnostics`]) and the exact PDF
//!   written at idle ([`Hub::written`]). `wait` answers from them, exact
//!   (§9.2).
//! - **Notes** (§9.3): `.phitex/notes.json`, written here only, anchors
//!   moved as the files change ([`crate::review_text`]).
//! - **Changes** (§9.4): the hunks of the files the build reads against
//!   the baseline (git's index, else `.phitex/baseline/`), accepted and
//!   rejected here; an attached editor's edits accepted as typed.
//!
//! Nothing here is on a build's path but [`fed`] (a hash of each project
//! file the engine is given) and [`Hub::built`] after the viewers are
//! told; the events go out with the state lock let go.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::review_text as rt;
use crate::view::Ask;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A file's bytes' hash (a build's record, `wait`'s comparison).
pub fn hash(b: &[u8]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

/// Now, as notes say it (ISO 8601, UTC, to the second).
fn now() -> String {
    let t = jiff::Timestamp::now();
    t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The files reviewed: the project's text files (what a person edits).
fn reviewed(name: &str) -> bool {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "tex"
            | "bib"
            | "sty"
            | "cls"
            | "ltx"
            | "cfg"
            | "bbx"
            | "cbx"
            | "lbx"
            | "def"
            | "txt"
            | "md"
    ) && !name.starts_with(".phitex/")
}

// ---- notes.json ----

/// A note's place in the source (§9.3).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Anchor {
    pub file: String,
    pub start: usize,
    pub end: usize,
    #[serde(default)]
    pub quote: String,
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
    #[serde(default)]
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<[f64; 2]>,
}

/// A range the agent edited, as a resolve or reply recorded it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Edited {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub from_line: usize,
    pub to_line: usize,
}

/// A hunk as a comment, a verdict or an update names it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct HunkRef {
    pub id: String,
    pub file: String,
    pub line: usize,
    pub old: String,
    pub new: String,
}

/// One post of a thread.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Post {
    pub by: String,
    pub text: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edited: Vec<Edited>,
    /// A commented hunk's diff after the agent changed it ("updated").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<HunkRef>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub id: String,
    pub kind: String,
    pub anchor: Anchor,
    #[serde(default)]
    pub detached: bool,
    pub status: String,
    pub created: String,
    pub updated: String,
    #[serde(default)]
    pub thread: Vec<Post>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunk: Option<HunkRef>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ReviewRec {
    pub id: String,
    pub verdict: String,
    pub summary: String,
    pub at: String,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub hunks: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Verdict {
    pub hunk: HunkRef,
    pub verdict: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// Shown to the agent (`session changes`) already.
    #[serde(default)]
    pub seen: bool,
}

/// `.phitex/notes.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Store {
    pub version: u32,
    pub next: u64,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub reviews: Vec<ReviewRec>,
    #[serde(default)]
    pub verdicts: Vec<Verdict>,
}

impl Default for Store {
    fn default() -> Store {
        Store {
            version: 1,
            next: 1,
            notes: Vec::new(),
            reviews: Vec::new(),
            verdicts: Vec::new(),
        }
    }
}

/// The project's `.phitex/` made if missing; when it is made in a git
/// work tree, `.phitex/` is added to that repository's `.git/info/exclude`
/// (local to this clone, never a committed `.gitignore`) so git leaves it
/// out. The sentence to say so once, when it was added.
pub fn make_dir(root: &Path) -> Result<Option<String>, String> {
    let d = root.join(".phitex");
    if d.is_dir() {
        return Ok(None);
    }
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    Ok(exclude(root))
}

/// `.phitex/` in the `info/exclude` of the git repository `root` is in;
/// the sentence saying so, when it was added now.
fn exclude(root: &Path) -> Option<String> {
    let o = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ])
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let path = PathBuf::from(String::from_utf8_lossy(&o.stdout).trim());
    let now = std::fs::read_to_string(&path).unwrap_or_default();
    if now
        .lines()
        .any(|l| matches!(l.trim(), ".phitex/" | ".phitex" | "/.phitex/" | "/.phitex"))
    {
        return None;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let sep = if now.is_empty() || now.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let line = "# PhiTeX's notes, review baseline and session log (phitex session)\n.phitex/\n";
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{sep}{line}").as_bytes()))
        .ok()?;
    Some(format!(
        "made .phitex/ (notes, review baseline, session log) and added it to {}, so git leaves it out (this clone only)",
        path.display()
    ))
}

impl Store {
    /// The notes file of project `root`.
    pub fn path(root: &Path) -> PathBuf {
        root.join(".phitex").join("notes.json")
    }

    /// The project's notes (none yet: empty).
    pub fn load(root: &Path) -> Result<Store, String> {
        let p = Store::path(root);
        match std::fs::read(&p) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    /// Written whole: beside, then renamed.
    pub fn save(&self, root: &Path) -> Result<(), String> {
        let p = Store::path(root);
        if let Some(said) = make_dir(root)? {
            eprintln!("phitex: {said}");
        }
        let tmp = p.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("{}: {e}", p.display()))
    }

    fn id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}{}", self.next);
        self.next += 1;
        id
    }

    pub fn note(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    fn note_mut(&mut self, id: &str) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }
}

// ---- the baseline ----

/// Where accepted text is kept (§9.4).
enum Baseline {
    /// The project's git repository: its index. `dir` the work tree's
    /// folder for the project (git's commands run there), `index` the
    /// index file (its stamp says when another program staged).
    Git { dir: PathBuf, index: PathBuf },
    /// `.phitex/baseline/` in the project.
    Dir(PathBuf),
}

impl Baseline {
    fn of(root: &Path) -> Baseline {
        if phitex_git::Repo::discover(root).is_ok()
            && let Ok(o) = Command::new("git")
                .args(["rev-parse", "--absolute-git-dir"])
                .current_dir(root)
                .output()
            && o.status.success()
        {
            let gd = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            return Baseline::Git {
                dir: root.to_path_buf(),
                index: PathBuf::from(gd).join("index"),
            };
        }
        Baseline::Dir(root.join(".phitex").join("baseline"))
    }

    fn git(&self) -> bool {
        matches!(self, Baseline::Git { .. })
    }

    /// The index's stamp (none: not git, or no index yet).
    fn stamp(&self) -> Option<std::time::SystemTime> {
        match self {
            Baseline::Git { index, .. } => std::fs::metadata(index).and_then(|m| m.modified()).ok(),
            Baseline::Dir(_) => None,
        }
    }

    /// File `name`'s accepted text (none: the baseline has no such file).
    fn read(&self, name: &str) -> Option<String> {
        match self {
            Baseline::Git { dir, .. } => {
                let repo = phitex_git::Repo::discover(dir).ok()?;
                let s = repo.snapshot(phitex_git::STAGED).ok()?;
                phitex_diff::Files::read(&s, name)
            }
            Baseline::Dir(d) => std::fs::read(d.join(name))
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
        }
    }

    /// File `name`'s accepted text set to `text` (none: taken out).
    fn write(&self, name: &str, text: Option<&str>) -> Result<(), String> {
        match self {
            Baseline::Git { dir, .. } => {
                let git = |args: &[&str], input: Option<&str>| -> Result<String, String> {
                    let mut c = Command::new("git");
                    c.args(args).current_dir(dir);
                    c.stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    let mut ch = c.spawn().map_err(|e| format!("git: {e}"))?;
                    if let Some(t) = input
                        && let Some(mut i) = ch.stdin.take()
                    {
                        use std::io::Write as _;
                        i.write_all(t.as_bytes()).map_err(|e| format!("git: {e}"))?;
                    }
                    drop(ch.stdin.take());
                    let o = ch.wait_with_output().map_err(|e| format!("git: {e}"))?;
                    if !o.status.success() {
                        return Err(format!(
                            "git {}: {}",
                            args.join(" "),
                            String::from_utf8_lossy(&o.stderr).trim()
                        ));
                    }
                    Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned())
                };
                match text {
                    Some(t) => {
                        let id = git(&["hash-object", "-w", "--stdin"], Some(t))?;
                        let staged = git(&["ls-files", "-s", "--", name], None)?;
                        let mode = staged
                            .split_whitespace()
                            .next()
                            .unwrap_or("100644")
                            .to_owned();
                        git(
                            &[
                                "update-index",
                                "--add",
                                "--cacheinfo",
                                &format!("{mode},{id},{name}"),
                            ],
                            None,
                        )?;
                    }
                    None => {
                        git(&["update-index", "--force-remove", "--", name], None)?;
                    }
                }
                Ok(())
            }
            Baseline::Dir(d) => {
                let p = d.join(name);
                match text {
                    Some(t) => {
                        if let Some(root) = d.parent().and_then(Path::parent)
                            && let Some(said) = make_dir(root)?
                        {
                            eprintln!("phitex: {said}");
                        }
                        if let Some(parent) = p.parent() {
                            std::fs::create_dir_all(parent)
                                .map_err(|e| format!("{}: {e}", parent.display()))?;
                        }
                        std::fs::write(&p, t).map_err(|e| format!("{}: {e}", p.display()))
                    }
                    None => match std::fs::remove_file(&p) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                            Err(format!("{}: {e}", p.display()))
                        }
                        _ => Ok(()),
                    },
                }
            }
        }
    }
}

/// The baseline as Compare's versions (§9.4 "Show changes"): git's index
/// is `staged`; `.phitex/baseline/`'s files, over the disk's for the files
/// it does not hold, is this.
pub struct BaselineFiles(pub PathBuf);

impl phitex_diff::Files for BaselineFiles {
    fn read(&self, path: &str) -> Option<String> {
        let b = self.0.join(".phitex").join("baseline").join(path);
        std::fs::read(&b)
            .or_else(|_| std::fs::read(self.0.join(path)))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

impl crate::compare::Versions for BaselineFiles {
    fn files(&self, _rev: &str) -> Result<Box<dyn phitex_diff::Files>, String> {
        Ok(Box::new(BaselineFiles(self.0.clone())))
    }

    fn log(&self, _skip: usize, _limit: usize) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
}

// ---- the hub ----

/// What one build read and made (§9.2).
#[derive(Clone, Debug, Default)]
pub struct BuildRec {
    pub n: usize,
    pub ok: bool,
    pub pages: usize,
    pub ms: f64,
    /// Its errors as the terminal says them (`file:line: message`).
    pub errors: Vec<String>,
    /// The project's files it read: their hashes (none: looked for and
    /// not found).
    pub read: HashMap<String, Option<u64>>,
    /// Its diagnostics made (pending: when the editing pauses).
    pub diags_pending: bool,
    /// The PDF file it wrote, if any.
    pub pdf: Option<String>,
}

/// What a build gives the hub ([`Hub::built`]).
pub struct BuildFacts {
    pub n: usize,
    pub ok: bool,
    pub pages: usize,
    pub ms: f64,
    pub errors: Vec<String>,
    /// The main file's name and the bytes it built.
    pub main: (String, Option<Vec<u8>>),
    /// The files it reads, from the root (`Session::watched`): found, and
    /// names not found.
    pub paths: Vec<String>,
    pub missing: Vec<String>,
    pub diags_pending: bool,
    pub diags: Option<Vec<Value>>,
    pub pdf: Option<String>,
}

/// A hunk now: [`rt::Hunk`] in a file, named.
#[derive(Clone, Debug)]
struct HunkNow {
    id: String,
    file: String,
    line: usize,
    end_line: usize,
    base: (usize, usize),
    now: (usize, usize),
    parts: Vec<(rt::Part, String)>,
    old: String,
    new: String,
    /// The whole file added (no baseline) or taken out (not on disk).
    whole: Option<&'static str>,
}

impl HunkNow {
    fn as_ref(&self) -> HunkRef {
        HunkRef {
            id: self.id.clone(),
            file: self.file.clone(),
            line: self.line,
            old: self.old.clone(),
            new: self.new.clone(),
        }
    }
}

/// A file followed: its text now (an editor's buffer, else the disk's),
/// its accepted text, its hunks.
#[derive(Default)]
struct Tracked {
    text: Option<String>,
    /// The text's hash, as a build reads it.
    hash: Option<u64>,
    /// Its accepted text, once read (inside: none, the baseline has no
    /// such file).
    #[allow(clippy::option_option, reason = "not read yet, and read: no such file")]
    base: Option<Option<String>>,
    hunks: Vec<HunkNow>,
    /// The accepted text `hunks` were made from, with `text`: both the
    /// same, the same hunks (a build reads every file followed; one
    /// unchanged is not diffed again: Grail's 19, 5 ms a keystroke).
    #[allow(clippy::option_option, reason = "as `base`")]
    hunks_base: Option<Option<String>>,
}

/// The editors' side, given by the language server ([`Hub::on_editors`]):
/// a file's buffer where an editor has it open, and a buffer replaced.
pub struct Editors {
    pub buffer: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
    pub replace: Box<dyn Fn(&str, &str) -> bool + Send + Sync>,
}

#[derive(Default)]
struct State {
    root: PathBuf,
    main: String,
    viewer: Option<String>,
    /// What the watch runs and why: `engine`, `engine_why`, `out_dir`,
    /// `out_dir_why` ([`Hub::about`]), in the status.
    about: Value,
    started: bool,
    /// The watch ends (its socket closes too, which the CLI sees).
    ended: bool,
    /// The watch writes its PDF (else `wait --pdf` is refused).
    writing: bool,
    builds: VecDeque<BuildRec>,
    /// The project files the engine was given, by name: their hashes.
    fed: HashMap<String, u64>,
    /// The diagnostics of build `diags.0` (made).
    diags: (usize, Vec<Value>),
    /// The build whose exact PDF was written last.
    written: usize,
    transcript: (usize, String),
    /// What `session pdf` asked of a watch that writes no file came to,
    /// with the build it was of (and how many were asked, to tell them).
    pdf_out: (u64, usize, Option<Result<Value, String>>),
    store: Store,
    /// Every change to notes, reviews, verdicts: what `wait --notes`
    /// waits on (the user's), by sequence.
    seq: u64,
    user_events: Vec<(u64, String)>,
    /// The texts the agent read the notes against (`notes` listed).
    read: Option<Arc<HashMap<String, String>>>,
    files: HashMap<String, Tracked>,
    baseline: Option<Baseline>,
    base_stamp: Option<std::time::SystemTime>,
    /// The editors' buffers as last told, by file.
    buffers: HashMap<String, String>,
    /// Accepted edits to write to git's index (the user's typing).
    base_dirty: HashSet<String>,
    accept_disk: bool,
    /// The first build is in: a file the baseline folder has not seen
    /// is one the agent made (an added file), not the project's as found.
    seen_first: bool,
    /// Reviewing the agent's edits ([`Hub::set_changes`]): off, the files
    /// are followed for the notes' anchors alone.
    changes: bool,
}

/// Said where reviewing the agent's edits is asked for while it is off.
pub const CHANGES_OFF: &str = "reviewing your edits is off (an experimental feature): the user turns it on in the viewer's Settings, Experimental, or with `phitex config experimental-review on`";

/// Status codes `phitex session` exits with (§9.1).
pub const EXIT_TIMEOUT: i32 = 2;
pub const EXIT_NO_SESSION: i32 = 3;
pub const EXIT_REFUSED: i32 = 4;

type Emit = Box<dyn Fn(&Value) + Send + Sync>;
type Asker = Box<dyn Fn(Ask) + Send + Sync>;
type Quit = Box<dyn Fn() + Send + Sync>;

/// The review's state, shared by the watch's threads.
#[derive(Default)]
pub struct Hub {
    st: Mutex<State>,
    cv: Condvar,
    emit: Mutex<Option<Arc<Emit>>>,
    ask: Mutex<Option<Arc<Asker>>>,
    quit: Mutex<Option<Arc<Quit>>>,
    editors: Mutex<Option<Arc<Editors>>>,
}

static HUB: OnceLock<Arc<Hub>> = OnceLock::new();

/// The process's hub (one watch a process).
pub fn hub() -> &'static Arc<Hub> {
    HUB.get_or_init(Arc::default)
}

/// The host gave the engine `bytes` as `path` (`Host::fed`): its hash
/// kept where it is a project file (under the root).
pub fn fed(path: &Path, bytes: &[u8]) {
    let Some(h) = HUB.get() else {
        return;
    };
    let name = {
        let st = lock(&h.st);
        if !st.started {
            return;
        }
        match name_in(&st.root, path) {
            Some(n) => n,
            None => return,
        }
    };
    let x = hash(bytes);
    lock(&h.st).fed.insert(name, x);
}

/// `path` as a project file's name (from `root`), none outside it.
fn name_in(root: &Path, path: &Path) -> Option<String> {
    let rel = if path.is_absolute() {
        path.strip_prefix(root).ok()?.to_path_buf()
    } else {
        path.to_path_buf()
    };
    let s = rel.to_string_lossy();
    let s = s.strip_prefix("./").unwrap_or(&s);
    if s.starts_with("../") || s.is_empty() {
        return None;
    }
    Some(s.replace('\\', "/"))
}

/// A request's failure: JSON-RPC's code and message, and the status the
/// CLI exits with.
#[derive(Debug, Clone)]
pub struct Fail {
    pub status: i32,
    pub message: String,
}

fn refused(m: impl Into<String>) -> Fail {
    Fail {
        status: EXIT_REFUSED,
        message: m.into(),
    }
}

impl Hub {
    /// The watch runs in `root` on `main` (from the root); notes read.
    pub fn start(&self, root: &Path, main: &str, writing: bool, accept_disk: bool) {
        let mut st = lock(&self.st);
        if st.root != root {
            st.store = Store::load(root).unwrap_or_default();
            st.baseline = None;
            st.files.clear();
            st.seen_first = false;
        }
        root.clone_into(&mut st.root);
        main.clone_into(&mut st.main);
        st.writing = writing;
        st.accept_disk = accept_disk;
        st.started = true;
        st.ended = false;
    }

    /// Reviewing the agent's edits on or off (`experimental-review`, off
    /// by default; the viewer's Settings, Experimental). Off, nothing of
    /// it is done: no baseline read, no hunks, nothing written; turned on,
    /// the files' texts as they are now are the accepted ones where there
    /// is no git (git's index is, where there is).
    pub fn set_changes(&self, on: bool) {
        let mut st = lock(&self.st);
        if st.changes == on {
            return;
        }
        st.changes = on;
        if on {
            let root = st.root.clone();
            st.baseline = Some(Baseline::of(&root));
            let names: Vec<String> = st.files.keys().cloned().collect();
            for n in names {
                let text = st.files.get(&n).and_then(|t| t.text.clone());
                if let Some(Baseline::Dir(d)) = &st.baseline
                    && !d.join(&n).exists()
                    && let Some(t) = &text
                {
                    st.files.entry(n.clone()).or_default().base = Some(Some(t.clone()));
                } else if let Some(t) = st.files.get_mut(&n) {
                    t.base = None;
                }
                refresh(&mut st, &n, text);
            }
        } else {
            st.baseline = None;
            st.base_dirty.clear();
            for t in st.files.values_mut() {
                t.hunks.clear();
                t.base = None;
            }
        }
        drop(st);
        self.cv.notify_all();
        self.tell();
    }

    /// The engine and the output folder the watch runs, each with how it
    /// was chosen (`{engine, engine_why, out_dir, out_dir_why}`).
    pub fn about(&self, v: Value) {
        lock(&self.st).about = v;
    }

    pub fn set_viewer(&self, url: &str) {
        lock(&self.st).viewer = Some(url.to_owned());
    }

    /// Events (the review's state) go to `emit` (every viewer).
    pub fn on_emit(&self, emit: Emit) {
        *lock(&self.emit) = Some(Arc::new(emit));
    }

    /// What the hub asks of the watch (rebuild, the transcript) and how it
    /// ends it.
    pub fn on_ask(&self, ask: Asker, quit: Quit) {
        *lock(&self.ask) = Some(Arc::new(ask));
        *lock(&self.quit) = Some(Arc::new(quit));
    }

    pub fn on_editors(&self, e: Editors) {
        *lock(&self.editors) = Some(Arc::new(e));
    }

    fn ask_watch(&self, a: Ask) {
        let f = lock(&self.ask).clone();
        if let Some(f) = f {
            f(a);
        }
    }

    fn editors(&self) -> Option<Arc<Editors>> {
        lock(&self.editors).clone()
    }

    /// The review's state to every viewer (the lock let go).
    fn tell(&self) {
        let v = {
            let st = lock(&self.st);
            state_json(&st)
        };
        let e = lock(&self.emit).clone();
        if let Some(e) = e {
            e(&v);
        }
    }

    /// A build is in (after the viewers were told): its record made from
    /// what the engine was given, the files followed brought up to date.
    pub fn built(&self, f: BuildFacts) {
        let editors = self.editors();
        let mut st = lock(&self.st);
        let mut read = HashMap::new();
        for p in &f.paths {
            if let Some(n) = name_in(&st.root, Path::new(p))
                && let Some(h) = st.fed.get(&n)
            {
                read.insert(n, Some(*h));
            }
        }
        if let (main, Some(b)) = &f.main {
            read.insert(main.clone(), Some(hash(b)));
        }
        for m in &f.missing {
            let n = m.strip_prefix("./").unwrap_or(m).to_owned();
            if !read.contains_key(&n) && reviewed(&n) {
                read.insert(n, None);
            }
        }
        let rec = BuildRec {
            n: f.n,
            ok: f.ok,
            pages: f.pages,
            ms: f.ms,
            errors: f.errors,
            read,
            diags_pending: f.diags_pending,
            pdf: f.pdf,
        };
        if let Some(d) = f.diags {
            st.diags = (f.n, d);
        }
        let names: Vec<String> = rec
            .read
            .iter()
            .filter(|(n, h)| h.is_some() && reviewed(n))
            .map(|(n, _)| n.clone())
            .collect();
        st.builds.push_back(rec);
        while st.builds.len() > 32 {
            st.builds.pop_front();
        }
        // (the files followed: those the build read, and those notes or
        // the baseline name)
        let mut follow: HashSet<String> = names.into_iter().collect();
        follow.extend(st.files.keys().cloned());
        follow.extend(st.store.notes.iter().map(|n| n.anchor.file.clone()));
        let restaged = st.baseline.as_ref().and_then(Baseline::stamp);
        if restaged != st.base_stamp {
            st.base_stamp = restaged;
            for t in st.files.values_mut() {
                t.base = None;
            }
        }
        for n in follow {
            let text = editors.as_ref().and_then(|e| (e.buffer)(&n)).or_else(|| {
                std::fs::read(st.root.join(&n))
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
            });
            refresh(&mut st, &n, text);
        }
        flush_base(&mut st);
        st.seen_first = true;
        drop(st);
        self.cv.notify_all();
        self.tell();
    }

    /// The diagnostics left for later, made now (of build `n`).
    pub fn diagnostics(&self, n: usize, d: &[Value]) {
        let mut st = lock(&self.st);
        st.diags = (n, d.to_vec());
        if let Some(b) = st.builds.iter_mut().rev().find(|b| b.n <= n) {
            b.diags_pending = false;
        }
        drop(st);
        self.cv.notify_all();
    }

    /// Build `n`'s exact PDF is on disk.
    pub fn written(&self, n: usize) {
        lock(&self.st).written = n;
        self.cv.notify_all();
    }

    /// `session pdf`'s write (of build `n`) done by the watch.
    pub fn pdf_written(&self, n: usize, r: Result<Value, String>) {
        let mut st = lock(&self.st);
        st.pdf_out.1 = n;
        st.pdf_out.2 = Some(r);
        st.pdf_out.0 += 1;
        drop(st);
        self.cv.notify_all();
    }

    /// Build `n`'s transcript.
    pub fn transcript(&self, n: usize, text: String) {
        lock(&self.st).transcript = (n, text);
        self.cv.notify_all();
    }

    // ---- the editors' edits (the user's) ----

    /// An attached editor's change of `file` (UTF-16 `start..end` of its
    /// buffer replaced by `text`; none: the whole buffer is `text`): the
    /// user's, accepted at once where it is outside every pending hunk.
    /// `open`: the editor opened it (no edit).
    pub fn user_edit(&self, file: &str, range: Option<(usize, usize)>, text: &str, open: bool) {
        let file = file.strip_prefix("./").unwrap_or(file).to_owned();
        if !reviewed(&file) {
            return;
        }
        let mut st = lock(&self.st);
        if !st.started {
            return;
        }
        let open = open || !st.buffers.contains_key(&file);
        let before = st
            .buffers
            .get(&file)
            .cloned()
            .or_else(|| st.files.get(&file).and_then(|t| t.text.clone()));
        let after = match (range, &before) {
            (Some((a, b)), Some(t)) => phitex_live::watch::splice_utf16(t, a, b, text),
            _ => text.to_owned(),
        };
        st.buffers.insert(file.clone(), after.clone());
        let Some(before) = before else {
            return;
        };
        if open || before == after {
            return;
        }
        // (the editor taking the disk's text, the agent's write: not the user's)
        let disk = std::fs::read(st.root.join(&file))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        if disk.as_deref() == Some(after.as_str()) {
            refresh(&mut st, &file, Some(after));
            return;
        }
        if st.changes {
            accept_typed(&mut st, &file, &before, &after);
        }
        refresh(&mut st, &file, Some(after));
    }

    /// An editor closed `file`: the disk's again.
    pub fn user_closed(&self, file: &str) {
        let file = file.strip_prefix("./").unwrap_or(file);
        lock(&self.st).buffers.remove(file);
    }

    // ---- requests (the session's CLI, editors) ----

    /// A `phitex/*` request on the attach socket (§9.1): its result, or
    /// none (no such method).
    pub fn request(&self, method: &str, p: &Value) -> Option<Result<Value, Fail>> {
        Some(match method {
            "phitex/session/status" => Ok(self.status(false)),
            "phitex/session/wait" => self.wait(p),
            "phitex/session/problems" => Ok(self.problems(p)),
            "phitex/session/rebuild" => self.rebuild(p),
            "phitex/session/log" => self.log(p),
            "phitex/session/pdf" => self.pdf(p),
            "phitex/session/stop" => {
                let q = lock(&self.quit).clone();
                if let Some(q) = q {
                    q();
                }
                Ok(Value::Null)
            }
            "phitex/notes/list" => Ok(self.list(p)),
            "phitex/notes/reply" => self.reply(p, "agent"),
            "phitex/notes/resolve" => self.resolve(p, "agent"),
            "phitex/changes/list" => Ok(self.changes()),
            _ => return None,
        })
    }

    fn status_of(st: &State, b: Option<&BuildRec>, warnings: bool) -> Value {
        let problems: Vec<Value> = st
            .diags
            .1
            .iter()
            .filter(|d| d["severity"] == "error" || (warnings && d["severity"] == "warning"))
            .cloned()
            .collect();
        let count = |sev: &str| st.diags.1.iter().filter(|d| d["severity"] == sev).count();
        // (the build's own errors where no diagnostic placed them)
        let problems = if problems.is_empty() && b.is_some_and(|b| !b.errors.is_empty()) {
            b.map(|b| {
                b.errors
                    .iter()
                    .map(|e| json!({"severity": "error", "message": e}))
                    .collect()
            })
            .unwrap_or_default()
        } else {
            problems
        };
        json!({
            "build": b.map(|b| b.n),
            "ok": b.map(|b| b.ok),
            "errors": b.map_or(0, |b| b.errors.len()),
            "warnings": count("warning"),
            "pages": b.map(|b| b.pages),
            "ms": b.map(|b| b.ms),
            "pdf": b.and_then(|b| b.pdf.clone()),
            "viewer": st.viewer,
            "main": st.main,
            "root": st.root.display().to_string(),
            "pid": std::process::id(),
            "engine": st.about["engine"],
            "engine_why": st.about["engine_why"],
            "out_dir": st.about["out_dir"],
            "out_dir_why": st.about["out_dir_why"],
            "changes": st.changes,
            "problems": problems,
            "diagnostics_pending": b.is_some_and(|b| b.diags_pending),
            "written": b.is_some_and(|b| st.written >= b.n),
        })
    }

    fn status(&self, warnings: bool) -> Value {
        let st = lock(&self.st);
        Hub::status_of(&st, st.builds.back(), warnings)
    }

    fn problems(&self, p: &Value) -> Value {
        let st = lock(&self.st);
        let errors = p["errors"].as_bool().unwrap_or(false);
        let warnings = p["warnings"].as_bool().unwrap_or(false);
        let file = p["file"].as_str();
        let all = !errors && !warnings;
        let list: Vec<Value> = st
            .diags
            .1
            .iter()
            .filter(|d| {
                let sev = d["severity"].as_str().unwrap_or("");
                (all && sev != "diagnostic")
                    || (errors && sev == "error")
                    || (warnings && sev == "warning")
            })
            .filter(|d| {
                file.is_none_or(|f| {
                    d["file"].as_str().map(|x| x.trim_start_matches("./"))
                        == Some(f.trim_start_matches("./"))
                })
            })
            .cloned()
            .collect();
        json!({"build": st.diags.0, "problems": list})
    }

    /// The hashes of the files the last build read, as they are now.
    fn now_hashes(st: &State, editors: Option<&Editors>) -> HashMap<String, Option<u64>> {
        let Some(last) = st.builds.back() else {
            return HashMap::new();
        };
        last.read
            .keys()
            .map(|n| {
                let h = editors
                    .and_then(|e| (e.buffer)(n))
                    .map(|t| hash(t.as_bytes()))
                    .or_else(|| std::fs::read(st.root.join(n)).ok().map(|b| hash(&b)));
                (n.clone(), h)
            })
            .collect()
    }

    /// `wait` (§9.2).
    fn wait(&self, p: &Value) -> Result<Value, Fail> {
        let t0 = Instant::now();
        let timeout = p["timeout_ms"]
            .as_u64()
            .filter(|&t| t > 0)
            .map(Duration::from_millis);
        let until = timeout.map(|d| t0 + d);
        if p["notes"].as_bool().unwrap_or(false) {
            return self.wait_notes(until);
        }
        let warnings = p["warnings"].as_bool().unwrap_or(false);
        let pdf = p["pdf"].as_bool().unwrap_or(false);
        let editors = self.editors();
        let mut st = lock(&self.st);
        if pdf && !st.writing {
            return Err(refused(
                "this watch writes no PDF file (start it with --write, or `phitex config watch-write true`): `phitex session pdf` writes it now",
            ));
        }
        let want = Hub::now_hashes(&st, editors.as_deref());
        let first = st.builds.is_empty();
        // (the files an editor holds whose buffer is not the disk's)
        let held: Vec<String> = want
            .keys()
            .filter(|n| {
                editors
                    .as_ref()
                    .and_then(|e| (e.buffer)(n))
                    .is_some_and(|b| {
                        std::fs::read(st.root.join(n.as_str())).is_ok_and(|d| d != b.as_bytes())
                    })
            })
            .cloned()
            .collect();
        let matches = |b: &BuildRec| want.iter().all(|(n, h)| b.read.get(n) == Some(h));
        loop {
            if st.ended {
                return Err(Fail {
                    status: EXIT_NO_SESSION,
                    message: "the watch ended".into(),
                });
            }
            let found = st
                .builds
                .iter()
                .rev()
                .find(|b| (first || matches(b)) && !st.builds.is_empty())
                .cloned();
            if let Some(b) = found {
                let diags_ok = !warnings || !b.diags_pending;
                let pdf_ok = !pdf || st.written >= b.n;
                if diags_ok && pdf_ok {
                    let mut v = Hub::status_of(&st, Some(&b), warnings);
                    v["waited_ms"] = json!(t0.elapsed().as_secs_f64() * 1e3);
                    v["held"] = json!(held);
                    return Ok(v);
                }
            }
            st = match until {
                None => self.cv.wait(st).unwrap_or_else(PoisonError::into_inner),
                Some(u) => {
                    let left = u.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        let mut v = Hub::status_of(&st, st.builds.back(), warnings);
                        v["timed_out"] = json!(true);
                        v["held"] = json!(held);
                        return Ok(v);
                    }
                    self.cv
                        .wait_timeout(st, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    fn wait_notes(&self, until: Option<Instant>) -> Result<Value, Fail> {
        let mut st = lock(&self.st);
        let from = st.seq;
        loop {
            if st.ended {
                return Err(Fail {
                    status: EXIT_NO_SESSION,
                    message: "the watch ended".into(),
                });
            }
            let ids: Vec<String> = st
                .user_events
                .iter()
                .filter(|(s, _)| *s > from)
                .map(|(_, id)| id.clone())
                .collect();
            if !ids.is_empty() {
                let mut seen = HashSet::new();
                let notes: Vec<Value> = ids
                    .iter()
                    .filter(|id| seen.insert((*id).clone()))
                    .filter_map(|id| note_out(&st, id))
                    .collect();
                let reviews: Vec<Value> = ids
                    .iter()
                    .filter_map(|id| st.store.reviews.iter().find(|r| &r.id == id))
                    .map(|r| json!(r))
                    .collect();
                return Ok(json!({"notes": notes, "reviews": reviews}));
            }
            st = match until {
                None => self.cv.wait(st).unwrap_or_else(PoisonError::into_inner),
                Some(u) => {
                    let left = u.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(json!({"notes": [], "timed_out": true}));
                    }
                    self.cv
                        .wait_timeout(st, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    fn rebuild(&self, p: &Value) -> Result<Value, Fail> {
        let n0 = lock(&self.st).builds.back().map_or(0, |b| b.n);
        self.ask_watch(if p["clean"].as_bool().unwrap_or(false) {
            Ask::Cold
        } else {
            Ask::Rebuild
        });
        let mut st = lock(&self.st);
        loop {
            if st.ended {
                return Err(Fail {
                    status: EXIT_NO_SESSION,
                    message: "the watch ended".into(),
                });
            }
            if st.builds.back().is_some_and(|b| b.n > n0) {
                return Ok(Hub::status_of(&st, st.builds.back(), false));
            }
            st = self.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// `session pdf`: the last build's exact PDF and .synctex.gz on disk
    /// now. A watch that writes them (`--write`) has them written when it
    /// is idle: waited for, copied to `to` when asked; one that writes
    /// none is asked to write them now ([`Ask::WritePdf`]).
    fn pdf(&self, p: &Value) -> Result<Value, Fail> {
        let to = p["to"].as_str().map(str::to_owned);
        let synctex = p["synctex"].as_bool().unwrap_or(false);
        let mut st = lock(&self.st);
        let Some(n) = st.builds.back().map(|b| b.n) else {
            return Err(refused("no build yet: `phitex session wait` first"));
        };
        let ended = || Fail {
            status: EXIT_NO_SESSION,
            message: "the watch ended".into(),
        };
        if st.writing {
            loop {
                if st.ended {
                    return Err(ended());
                }
                let b = st.builds.back().map_or(n, |b| b.n);
                if st.written >= b {
                    break;
                }
                st = self.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
            let b = st.builds.back();
            let pdf = b
                .and_then(|b| b.pdf.clone())
                .ok_or_else(|| refused("the last build wrote no PDF"))?;
            let n = b.map_or(n, |b| b.n);
            let root = st.root.clone();
            drop(st);
            let pdf = root.join(pdf);
            // (the watch writes its .synctex.gz beside, at idle: said, and
            // copied with the PDF, only when asked)
            let gz = PathBuf::from(format!("{}.synctex.gz", pdf.with_extension("").display()));
            if synctex && !gz.exists() {
                return Err(refused(
                    "no .synctex.gz beside the PDF (`phitex config synctex on`)",
                ));
            }
            let pdf = match to {
                None => pdf,
                Some(t) => {
                    let t = root.join(t);
                    std::fs::copy(&pdf, &t)
                        .map_err(|e| refused(format!("{}: {e}", t.display())))?;
                    if synctex {
                        let tg =
                            PathBuf::from(format!("{}.synctex.gz", t.with_extension("").display()));
                        std::fs::copy(&gz, &tg)
                            .map_err(|e| refused(format!("{}: {e}", tg.display())))?;
                    }
                    t
                }
            };
            return Ok(json!({"build": n, "pdf": pdf.display().to_string(), "synctex": synctex}));
        }
        let asked = st.pdf_out.0;
        drop(st);
        self.ask_watch(Ask::WritePdf { to, synctex });
        let mut st = lock(&self.st);
        loop {
            if st.ended {
                return Err(ended());
            }
            if st.pdf_out.0 > asked {
                let n = st.pdf_out.1;
                return match st.pdf_out.2.clone() {
                    Some(Ok(mut v)) => {
                        v["build"] = json!(n);
                        Ok(v)
                    }
                    Some(Err(e)) => Err(refused(e)),
                    None => Err(refused("nothing written")),
                };
            }
            st = self.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn log(&self, p: &Value) -> Result<Value, Fail> {
        let last = lock(&self.st).builds.back().map_or(0, |b| b.n);
        if let Some(b) = p["build"].as_u64()
            && usize::try_from(b).ok() != Some(last)
        {
            return Err(refused(format!(
                "only the last build's log is kept (build {last})"
            )));
        }
        self.ask_watch(Ask::Transcript);
        let mut st = lock(&self.st);
        loop {
            if st.ended {
                return Err(Fail {
                    status: EXIT_NO_SESSION,
                    message: "the watch ended".into(),
                });
            }
            if st.transcript.0 >= last && last > 0 {
                return Ok(json!({"build": st.transcript.0, "text": st.transcript.1}));
            }
            let (g, to) = self
                .cv
                .wait_timeout(st, Duration::from_secs(30))
                .unwrap_or_else(PoisonError::into_inner);
            st = g;
            if to.timed_out() {
                return Err(Fail {
                    status: EXIT_TIMEOUT,
                    message: "the watch did not give its log".into(),
                });
            }
        }
    }

    /// `notes` (§9.3): the notes, marked read.
    fn list(&self, p: &Value) -> Value {
        let all = p["all"].as_bool().unwrap_or(false);
        let mut st = lock(&self.st);
        let snap: HashMap<String, String> = st
            .files
            .iter()
            .filter_map(|(n, t)| Some((n.clone(), t.text.clone()?)))
            .collect();
        st.read = Some(Arc::new(snap));
        let notes = st
            .store
            .notes
            .iter()
            .filter(|n| !n.draft && (all || n.status == "open"))
            .filter_map(|n| note_out(&st, &n.id))
            .collect::<Vec<_>>();
        json!({"notes": notes})
    }

    /// The ranges changed since the notes were read, by file.
    fn edited_since_read(st: &State) -> Vec<Edited> {
        let Some(then) = st.read.clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut names: Vec<&String> = st.files.keys().collect();
        names.sort();
        for n in names {
            let Some(now) = st.files.get(n).and_then(|t| t.text.as_ref()) else {
                continue;
            };
            let old = then.get(n).map_or("", String::as_str);
            for (s, e, a, b) in rt::edited(old, now) {
                out.push(Edited {
                    file: n.clone(),
                    start: s,
                    end: e,
                    from_line: a,
                    to_line: b,
                });
            }
        }
        out
    }

    fn post(&self, p: &Value, by: &str, resolve: Option<bool>) -> Result<Value, Fail> {
        let id = p["id"].as_str().unwrap_or("").to_owned();
        let text = p["text"].as_str().unwrap_or("").trim().to_owned();
        let mut st = lock(&self.st);
        if st.store.note(&id).is_none() {
            return Err(refused(format!("no note {id:?}")));
        }
        if text.is_empty() && resolve.is_none() {
            return Err(refused("a reply needs its text"));
        }
        let edited = if by == "agent" {
            Hub::edited_since_read(&st)
        } else {
            Vec::new()
        };
        let at = now();
        let n = st.store.note_mut(&id).expect("there");
        if !text.is_empty() || !edited.is_empty() {
            n.thread.push(Post {
                by: by.to_owned(),
                text,
                at: at.clone(),
                edited,
                diff: None,
            });
        }
        if let Some(r) = resolve {
            if r { "resolved" } else { "open" }.clone_into(&mut n.status);
        }
        n.updated = at;
        if by == "you" {
            bump(&mut st, &id);
        }
        save(&st);
        let out = note_out(&st, &id).unwrap_or(Value::Null);
        drop(st);
        self.cv.notify_all();
        self.tell();
        Ok(out)
    }

    fn reply(&self, p: &Value, by: &str) -> Result<Value, Fail> {
        self.post(p, by, None)
    }

    fn resolve(&self, p: &Value, by: &str) -> Result<Value, Fail> {
        self.post(p, by, Some(true))
    }

    /// `changes` (§9.4): pending hunks, the verdicts not yet seen, the
    /// comments on changes, the reviews.
    fn changes(&self) -> Value {
        let mut st = lock(&self.st);
        if !st.changes {
            return json!({"off": true, "pending": [], "verdicts": [], "comments": [], "reviews": [],
                          "how": CHANGES_OFF});
        }
        let pending: Vec<Value> = hunks_out(&st);
        let verdicts: Vec<Value> = st
            .store
            .verdicts
            .iter()
            .filter(|v| !v.seen)
            .map(|v| json!(v))
            .collect();
        for v in &mut st.store.verdicts {
            v.seen = true;
        }
        let comments: Vec<Value> = st
            .store
            .notes
            .iter()
            .filter(|n| n.hunk.is_some() && !n.draft && n.status == "open")
            .filter_map(|n| note_out(&st, &n.id))
            .collect();
        let reviews: Vec<Value> = st.store.reviews.iter().map(|r| json!(r)).collect();
        save(&st);
        json!({"pending": pending, "verdicts": verdicts, "comments": comments, "reviews": reviews, "git": st.baseline.as_ref().is_some_and(Baseline::git)})
    }

    // ---- the viewer's ops (§9.5) ----

    /// A `review_*` op of the viewer.
    pub fn viewer(&self, op: &str, req: &Value) -> Result<Value, String> {
        let r = match op {
            "review_state" => {
                let st = lock(&self.st);
                Ok(state_json(&st))
            }
            "review_note" => self.add_note(req),
            "review_reply" => self.reply(req, "you"),
            "review_resolve" => self.resolve(req, "you"),
            "review_reopen" => self.post(req, "you", Some(false)),
            "review_delete" => self.delete(req),
            "review_clear" => self.clear(),
            "review_accept" | "review_reject" | "review_comment" | "review_submit"
                if !lock(&self.st).changes =>
            {
                Err(refused(CHANGES_OFF))
            }
            "review_accept" => self.verdict(req, true, None),
            "review_reject" => self.verdict(req, false, None),
            "review_comment" => self.comment(req),
            "review_submit" => self.submit(req),
            "review_changes" => {
                self.set_changes(req["on"] == true);
                Ok(json!({"changes": lock(&self.st).changes}))
            }
            _ => return Err(format!("no op {op:?} here")),
        };
        r.map_err(|f| f.message)
    }

    fn add_note(&self, req: &Value) -> Result<Value, Fail> {
        let kind = req["kind"].as_str().unwrap_or("note");
        if !matches!(kind, "highlight" | "note" | "comment") {
            return Err(refused(format!("no kind {kind:?}")));
        }
        let a = &req["anchor"];
        let file = a["file"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches("./")
            .to_owned();
        let text = req["text"].as_str().unwrap_or("").trim().to_owned();
        if file.is_empty() {
            return Err(refused("a note needs its place (no glyph origins there)"));
        }
        if kind != "highlight" && text.is_empty() {
            return Err(refused("a note needs its words"));
        }
        let mut st = lock(&self.st);
        let n = |k: &str| {
            a[k].as_u64()
                .and_then(|x| usize::try_from(x).ok())
                .unwrap_or(0)
        };
        let (s, e) = (n("start"), n("end"));
        let body = current_text(&mut st, &file).unwrap_or_default();
        let s = rt::floor(&body, s);
        let e = rt::floor(&body, e.max(s));
        let (before, after) = rt::context(&body, (s, e));
        let anchor = Anchor {
            file: file.clone(),
            start: s,
            end: e,
            quote: body[s..e].to_owned(),
            before,
            after,
            line: rt::line_of(&body, s),
            page: a["page"].as_u64().and_then(|x| usize::try_from(x).ok()),
            point: a["point"]
                .as_array()
                .and_then(|p| Some([p.first()?.as_f64()?, p.get(1)?.as_f64()?])),
        };
        let id = st.store.id("n");
        let at = now();
        let thread = if text.is_empty() {
            Vec::new()
        } else {
            vec![Post {
                by: "you".into(),
                text,
                at: at.clone(),
                edited: Vec::new(),
                diff: None,
            }]
        };
        st.store.notes.push(Note {
            id: id.clone(),
            kind: kind.to_owned(),
            anchor,
            detached: false,
            status: "open".into(),
            created: at.clone(),
            updated: at,
            thread,
            hunk: None,
            draft: false,
            review: None,
        });
        st.files.entry(file).or_default();
        bump(&mut st, &id);
        save(&st);
        let out = note_out(&st, &id).unwrap_or(Value::Null);
        drop(st);
        self.cv.notify_all();
        self.tell();
        Ok(out)
    }

    fn delete(&self, req: &Value) -> Result<Value, Fail> {
        let id = req["id"].as_str().unwrap_or("");
        let mut st = lock(&self.st);
        let before = st.store.notes.len();
        st.store.notes.retain(|n| n.id != id);
        if st.store.notes.len() == before {
            return Err(refused(format!("no note {id:?}")));
        }
        save(&st);
        drop(st);
        self.tell();
        Ok(json!({"removed": 1}))
    }

    #[allow(clippy::unnecessary_wraps, reason = "one signature for the ops")]
    fn clear(&self) -> Result<Value, Fail> {
        let mut st = lock(&self.st);
        let before = st.store.notes.len();
        st.store.notes.retain(|n| n.status != "resolved");
        let removed = before - st.store.notes.len();
        save(&st);
        drop(st);
        self.tell();
        Ok(json!({"removed": removed}))
    }

    /// Accept (`yes`) or reject the hunks `req.ids` (`"all"`: every
    /// pending one, of `req.file` where given).
    fn verdict(&self, req: &Value, yes: bool, review: Option<&str>) -> Result<Value, Fail> {
        let editors = self.editors();
        let mut st = lock(&self.st);
        let file = req["file"].as_str();
        let all: Vec<HunkNow> = st
            .files
            .values()
            .flat_map(|t| t.hunks.iter().cloned())
            .filter(|h| file.is_none_or(|f| h.file == f))
            .collect();
        let chosen: Vec<HunkNow> = if req["ids"] == "all" {
            all
        } else {
            let ids: Vec<&str> = req["ids"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for id in &ids {
                if !all.iter().any(|h| h.id == *id) {
                    return Err(refused(
                        "this change was edited again since it was shown: look at it again",
                    ));
                }
            }
            all.into_iter()
                .filter(|h| ids.contains(&h.id.as_str()))
                .collect()
        };
        let mut by_file: HashMap<String, Vec<HunkNow>> = HashMap::new();
        for h in chosen {
            by_file.entry(h.file.clone()).or_default().push(h);
        }
        let mut done = 0;
        let at = now();
        let mut files: Vec<String> = by_file.keys().cloned().collect();
        files.sort();
        for f in files {
            let mut hs = by_file.remove(&f).unwrap_or_default();
            hs.sort_by_key(|h| h.base.0);
            let base = base_of(&mut st, &f);
            let cur = st.files.get(&f).and_then(|t| t.text.clone());
            if yes {
                // (the baseline with these hunks applied, from the last)
                let mut b = base.clone().unwrap_or_default();
                let mut gone = false;
                for h in hs.iter().rev() {
                    match h.whole {
                        Some("deleted") => gone = true,
                        Some(_) => b.clone_from(&h.new),
                        None => b.replace_range(h.base.0..h.base.1, &h.new),
                    }
                }
                let write = if gone { None } else { Some(b.as_str()) };
                st.baseline
                    .as_ref()
                    .expect("a baseline")
                    .write(&f, write)
                    .map_err(refused)?;
                if let Some(t) = st.files.get_mut(&f) {
                    t.base = Some(write.map(str::to_owned));
                }
            } else {
                // (the current text with these hunks' old text back, from the last)
                let owned = editors.as_ref().and_then(|e| (e.buffer)(&f));
                let disk = std::fs::read(st.root.join(&f))
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).into_owned());
                if let Some(buf) = &owned
                    && disk.as_deref() != Some(buf.as_str())
                {
                    return Err(refused(format!(
                        "{f} has unsaved changes in your editor: save or undo them there, then reject again"
                    )));
                }
                let mut t = cur.clone().unwrap_or_default();
                let mut gone = false;
                for h in hs.iter().rev() {
                    match h.whole {
                        Some("added") => gone = true,
                        Some(_) => t.clone_from(&h.old),
                        None => t.replace_range(h.now.0..h.now.1, &h.old),
                    }
                }
                let p = st.root.join(&f);
                if gone {
                    // (a new file rejected: kept aside, never deleted)
                    if let Ok(Some(said)) = make_dir(&st.root) {
                        eprintln!("phitex: {said}");
                    }
                    let aside = st.root.join(".phitex").join("rejected").join(&f);
                    if let Some(d) = aside.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    std::fs::rename(&p, &aside).map_err(|e| refused(format!("{f}: {e}")))?;
                } else {
                    if let Some(d) = p.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    std::fs::write(&p, &t).map_err(|e| refused(format!("{f}: {e}")))?;
                    if owned.is_some()
                        && let Some(e) = &editors
                    {
                        (e.replace)(&f, &t);
                    }
                }
                st.buffers.remove(&f);
            }
            for h in &hs {
                st.store.verdicts.push(Verdict {
                    hunk: h.as_ref(),
                    verdict: if yes { "accepted" } else { "rejected" }.into(),
                    at: at.clone(),
                    review: review.map(str::to_owned),
                    seen: false,
                });
                done += 1;
            }
            let text = if yes {
                cur
            } else {
                std::fs::read(st.root.join(&f))
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
            };
            refresh(&mut st, &f, text);
        }
        // (the verdicts kept: the last 500)
        let n = st.store.verdicts.len();
        if n > 500 {
            st.store.verdicts.drain(..n - 500);
        }
        st.seq += 1;
        save(&st);
        drop(st);
        self.cv.notify_all();
        self.tell();
        Ok(json!({"done": done}))
    }

    /// A comment on a hunk (`req.hunk` its id; `range`: bytes of the
    /// current text inside it; `draft`: part of a review not submitted).
    fn comment(&self, req: &Value) -> Result<Value, Fail> {
        let hid = req["hunk"].as_str().unwrap_or("");
        let text = req["text"].as_str().unwrap_or("").trim().to_owned();
        if text.is_empty() {
            return Err(refused("a comment needs its words"));
        }
        let mut st = lock(&self.st);
        let Some(h) = st
            .files
            .values()
            .flat_map(|t| t.hunks.iter())
            .find(|h| h.id == hid)
            .cloned()
        else {
            return Err(refused(
                "this change was edited again since it was shown: look at it again",
            ));
        };
        let body = st
            .files
            .get(&h.file)
            .and_then(|t| t.text.clone())
            .unwrap_or_default();
        let (s, e) = req["range"]
            .as_array()
            .and_then(|r| {
                let a = usize::try_from(r.first()?.as_u64()?).ok()?;
                let b = usize::try_from(r.get(1)?.as_u64()?).ok()?;
                (a >= h.now.0 && b <= h.now.1 && a <= b).then_some((a, b))
            })
            .unwrap_or(h.now);
        let (before, after) = rt::context(&body, (s, e));
        let id = st.store.id("n");
        let at = now();
        let draft = req["draft"].as_bool().unwrap_or(false);
        st.store.notes.push(Note {
            id: id.clone(),
            kind: "comment".into(),
            anchor: Anchor {
                file: h.file.clone(),
                start: s,
                end: e,
                quote: body.get(s..e).unwrap_or("").to_owned(),
                before,
                after,
                line: rt::line_of(&body, s),
                page: None,
                point: None,
            },
            detached: false,
            status: "open".into(),
            created: at.clone(),
            updated: at.clone(),
            thread: vec![Post {
                by: "you".into(),
                text,
                at,
                edited: Vec::new(),
                diff: None,
            }],
            hunk: Some(h.as_ref()),
            draft,
            review: None,
        });
        if !draft {
            bump(&mut st, &id);
        }
        save(&st);
        let out = note_out(&st, &id).unwrap_or(Value::Null);
        drop(st);
        self.cv.notify_all();
        self.tell();
        Ok(out)
    }

    /// A review submitted: the drafts published, the verdict on the hunks
    /// (`req.hunks`, else those the drafts are on; `"all"`: every pending).
    fn submit(&self, req: &Value) -> Result<Value, Fail> {
        let verdict = req["verdict"].as_str().unwrap_or("comment").to_owned();
        if !matches!(
            verdict.as_str(),
            "approve" | "request_changes" | "reject" | "comment"
        ) {
            return Err(refused(format!("no verdict {verdict:?}")));
        }
        let summary = req["summary"].as_str().unwrap_or("").trim().to_owned();
        let hunks: Vec<String> = {
            let mut st = lock(&self.st);
            let id = st.store.id("r");
            let drafts: Vec<String> = st
                .store
                .notes
                .iter()
                .filter(|n| n.draft)
                .map(|n| n.id.clone())
                .collect();
            if drafts.is_empty() && summary.is_empty() && verdict == "comment" {
                return Err(refused("a review needs a comment, a summary or a verdict"));
            }
            let pending: Vec<HunkNow> = st
                .files
                .values()
                .flat_map(|t| t.hunks.iter().cloned())
                .collect();
            let hunks: Vec<String> = if req["hunks"] == "all" {
                pending.iter().map(|h| h.id.clone()).collect()
            } else if let Some(a) = req["hunks"].as_array() {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            } else {
                let on: HashSet<String> = st
                    .store
                    .notes
                    .iter()
                    .filter(|n| n.draft)
                    .filter_map(|n| n.hunk.as_ref().map(|h| h.id.clone()))
                    .collect();
                pending
                    .iter()
                    .filter(|h| on.contains(&h.id))
                    .map(|h| h.id.clone())
                    .collect()
            };
            for n in &mut st.store.notes {
                if n.draft {
                    n.draft = false;
                    n.review = Some(id.clone());
                }
            }
            st.store.reviews.push(ReviewRec {
                id: id.clone(),
                verdict: verdict.clone(),
                summary,
                at: now(),
                notes: drafts,
                hunks: hunks.clone(),
            });
            bump(&mut st, &id);
            save(&st);
            hunks
        };
        if !hunks.is_empty() && (verdict == "approve" || verdict == "reject") {
            let last = lock(&self.st).store.reviews.last().map(|r| r.id.clone());
            self.verdict(
                &json!({"ids": hunks}),
                verdict == "approve",
                last.as_deref(),
            )?;
        }
        self.cv.notify_all();
        self.tell();
        let st = lock(&self.st);
        Ok(st.store.reviews.last().map_or(Value::Null, |r| json!(r)))
    }
}

/// The user did something the agent waits for (`wait --notes`).
fn bump(st: &mut State, id: &str) {
    st.seq += 1;
    let s = st.seq;
    st.user_events.push((s, id.to_owned()));
    if st.user_events.len() > 256 {
        st.user_events.drain(..128);
    }
}

/// The notes written, once there is something to keep (a note, a review,
/// a verdict) or the file is there: a watch with none writes nothing
/// into the project.
fn save(st: &State) {
    if st.root.as_os_str().is_empty() {
        return;
    }
    let s = &st.store;
    if s.notes.is_empty()
        && s.reviews.is_empty()
        && s.verdicts.is_empty()
        && !Store::path(&st.root).exists()
    {
        return;
    }
    let _ = s.save(&st.root);
}

/// File `name`'s text now (followed, else read).
fn current_text(st: &mut State, name: &str) -> Option<String> {
    if let Some(t) = st.files.get(name).and_then(|t| t.text.clone()) {
        return Some(t);
    }
    let t = st.buffers.get(name).cloned().or_else(|| {
        std::fs::read(st.root.join(name))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    });
    refresh(st, name, t.clone());
    t
}

/// File `name`'s accepted text (read once, until the index moves).
fn base_of(st: &mut State, name: &str) -> Option<String> {
    if let Some(b) = st.files.get(name).and_then(|t| t.base.clone()) {
        return b;
    }
    let b = st.baseline.as_ref().and_then(|b| b.read(name));
    st.files.entry(name.to_owned()).or_default().base = Some(b.clone());
    b
}

/// File `name` is `text` now (none: not there): its anchors moved, its
/// hunks made again, the comments on them told of an update.
fn refresh(st: &mut State, name: &str, text: Option<String>) {
    let old = st.files.get(name).and_then(|t| t.text.clone());
    // (a file the baseline folder has not seen, there when the watch
    // began: its text as found is the baseline, kept here; written into
    // the folder only once a change of it waits for review: a watch with
    // nothing to review writes nothing into the project)
    if !st.changes {
        // (the notes' anchors moved; nothing else of the review)
        if let (Some(o), Some(n)) = (&old, &text)
            && o != n
        {
            move_anchors(st, name, o, n);
        } else if old.is_none()
            && let Some(n) = &text
        {
            settle_anchors(st, name, n);
        }
        let t = st.files.entry(name.to_owned()).or_default();
        t.hash = text.as_ref().map(|x| hash(x.as_bytes()));
        t.text = text;
        t.hunks.clear();
        return;
    }
    if let Some(Baseline::Dir(d)) = &st.baseline
        && let Some(t) = &text
        && !st.seen_first
        && !d.join(name).exists()
        && st.files.get(name).is_none_or(|tr| tr.base.is_none())
    {
        st.files.entry(name.to_owned()).or_default().base = Some(Some(t.clone()));
    }
    if let (Some(o), Some(n)) = (&old, &text)
        && o != n
    {
        move_anchors(st, name, o, n);
    } else if old.is_none()
        && let Some(n) = &text
    {
        // (first seen: the anchors found by their quotes where they moved meanwhile)
        settle_anchors(st, name, n);
    }
    let base = base_of(st, name);
    let t = st.files.entry(name.to_owned()).or_default();
    if old != text || t.hunks_base.as_ref() != Some(&base) {
        t.hunks = make_hunks(name, base.as_deref(), text.as_deref());
        t.hunks_base = Some(base);
    }
    if old != text || t.hash.is_none() {
        t.hash = text.as_ref().map(|x| hash(x.as_bytes()));
    }
    t.text = text;
    let waiting = !t.hunks.is_empty();
    // (the baseline as found, written once a change waits: it outlives the watch)
    if waiting
        && let Some(Baseline::Dir(d)) = &st.baseline
        && !d.join(name).exists()
        && let Some(b) = st
            .files
            .get(name)
            .and_then(|t| t.hunks_base.clone().flatten())
    {
        let _ = st.baseline.as_ref().map(|bl| bl.write(name, Some(&b)));
    }
    update_comments(st, name);
    if st.accept_disk && !st.buffers.contains_key(name) {
        // (`review-accept-disk`: every change on disk taken as accepted)
        let pend: Vec<HunkNow> = st
            .files
            .get(name)
            .map(|t| t.hunks.clone())
            .unwrap_or_default();
        if !pend.is_empty()
            && let Some(t) = st.files.get(name).and_then(|t| t.text.clone())
            && st
                .baseline
                .as_ref()
                .is_some_and(|b| b.write(name, Some(&t)).is_ok())
            && let Some(tr) = st.files.get_mut(name)
        {
            tr.base = Some(Some(t));
            tr.hunks.clear();
        }
    }
}

/// A hunk's name: its file, its old and new text, and which of the
/// hunks of the file alike it is (not its place: accepting one before it
/// moves it in the baseline, and it is the same hunk).
fn hunk_id(file: &str, k: usize, old: &str, new: &str) -> String {
    let mut h = std::hash::DefaultHasher::new();
    (file, k, old, new).hash(&mut h);
    format!("h{:08x}", h.finish() & 0xffff_ffff)
}

fn make_hunks(file: &str, base: Option<&str>, text: Option<&str>) -> Vec<HunkNow> {
    match (base, text) {
        (None, None) => Vec::new(),
        (Some(b), None) => vec![HunkNow {
            id: hunk_id(file, 0, b, ""),
            file: file.to_owned(),
            line: 1,
            end_line: rt::line_of(b, b.len()),
            base: (0, b.len()),
            now: (0, 0),
            parts: vec![(rt::Part::Del, b.to_owned())],
            old: b.to_owned(),
            new: String::new(),
            whole: Some("deleted"),
        }],
        (None, Some(t)) => vec![HunkNow {
            id: hunk_id(file, 0, "", t),
            file: file.to_owned(),
            line: 1,
            end_line: rt::line_of(t, t.len()),
            base: (0, 0),
            now: (0, t.len()),
            parts: vec![(rt::Part::Add, t.to_owned())],
            old: String::new(),
            new: t.to_owned(),
            whole: Some("added"),
        }],
        (Some(b), Some(t)) => {
            let mut alike: HashMap<(String, String), usize> = HashMap::new();
            rt::hunks(b, t)
                .into_iter()
                .map(|h| {
                    let (old, new) = h.old_new();
                    let last = if h.b.1 > h.b.0 { h.b.1 - 1 } else { h.b.0 };
                    let k = alike.entry((old.clone(), new.clone())).or_default();
                    *k += 1;
                    HunkNow {
                        id: hunk_id(file, *k, &old, &new),
                        file: file.to_owned(),
                        line: rt::line_of(t, h.b.0),
                        end_line: rt::line_of(t, last),
                        base: h.a,
                        now: h.b,
                        parts: h.parts,
                        old,
                        new,
                        whole: None,
                    }
                })
                .collect()
        }
    }
}

/// The user's typing (`before` to `after` in an editor) applied to the
/// baseline where it is outside every pending hunk (§9.4).
fn accept_typed(st: &mut State, name: &str, before: &str, after: &str) {
    let Some(base) = base_of(st, name) else {
        return;
    };
    let pending = make_hunks(name, Some(&base), Some(before));
    let mut b = base;
    let mut moved = false;
    for c in rt::changes(before, after).iter().rev() {
        let touches = pending.iter().any(|h| c.a.0 <= h.now.1 && h.now.0 <= c.a.1);
        if touches {
            continue;
        }
        // (its place in the baseline: past the hunks before it)
        let delta: isize = pending
            .iter()
            .filter(|h| h.now.1 <= c.a.0)
            .map(|h| rt::growth(h.base, h.now))
            .sum();
        let s = c.a.0.saturating_add_signed(-delta);
        let e = c.a.1.saturating_add_signed(-delta);
        if e > b.len() || !b.is_char_boundary(s) || !b.is_char_boundary(e) {
            continue;
        }
        b.replace_range(s..e, &after[c.b.0..c.b.1]);
        moved = true;
    }
    if moved {
        let git = st.baseline.as_ref().is_some_and(Baseline::git);
        // (the folder's copy written where there is one: else the text
        // as found is still in memory only, nothing to review yet)
        if !git
            && let Some(bl @ Baseline::Dir(d)) = &st.baseline
            && d.join(name).exists()
        {
            let _ = bl.write(name, Some(&b));
        }
        st.files.entry(name.to_owned()).or_default().base = Some(Some(b));
        if git {
            st.base_dirty.insert(name.to_owned());
        }
    }
}

/// The user's accepted typing written to git's index (after a build, on
/// a thread of its own: the index is the user's, written when the
/// editing pauses).
fn flush_base(st: &mut State) {
    if st.base_dirty.is_empty() {
        return;
    }
    let names: Vec<String> = st.base_dirty.drain().collect();
    let Some(Baseline::Git { dir, index }) = &st.baseline else {
        return;
    };
    let b = Baseline::Git {
        dir: dir.clone(),
        index: index.clone(),
    };
    let texts: Vec<(String, Option<String>)> = names
        .into_iter()
        .filter_map(|n| {
            let t = st.files.get(&n)?.base.clone()?;
            Some((n, t))
        })
        .collect();
    let _ = std::thread::Builder::new()
        .name("phitex-review-index".into())
        .spawn(move || {
            static ONE: Mutex<()> = Mutex::new(());
            let _g = lock(&ONE);
            for (n, t) in texts {
                let _ = b.write(&n, t.as_deref());
            }
        });
}

/// The anchors and edited ranges of file `name` carried from `old` to `new`.
fn move_anchors(st: &mut State, name: &str, old: &str, new: &str) {
    let cs = rt::changes(old, new);
    for n in &mut st.store.notes {
        if n.anchor.file == name && !n.detached {
            let a = &mut n.anchor;
            let moved = if a.start == a.end {
                Some((rt::map_at(&cs, a.start), rt::map_at(&cs, a.start)))
            } else {
                rt::map_range(&cs, (a.start, a.end))
            };
            // (a comment on a change follows the change's place, never a
            // quote found elsewhere: the same words changed alike elsewhere
            // are another change)
            let moved = moved.or_else(|| {
                if n.hunk.is_some() {
                    let x = rt::map_at(&cs, a.start);
                    Some((x, x))
                } else {
                    rt::find_quote(new, &a.quote, &a.before, &a.after, a.start)
                }
            });
            match moved {
                Some((s, e)) => {
                    let s = rt::floor(new, s);
                    let e = rt::floor(new, e.max(s));
                    a.start = s;
                    a.end = e;
                    if !a.quote.is_empty() || e > s {
                        new[s..e].clone_into(&mut a.quote);
                    }
                    let (b, f) = rt::context(new, (s, e));
                    a.before = b;
                    a.after = f;
                    a.line = rt::line_of(new, s);
                }
                None => n.detached = true,
            }
        }
        for p in &mut n.thread {
            for e in &mut p.edited {
                if e.file == name
                    && let Some((s, t)) = rt::map_range(&cs, (e.start, e.end)).or_else(|| {
                        let x = rt::map_at(&cs, e.start);
                        Some((x, x))
                    })
                {
                    e.start = s;
                    e.end = t;
                }
            }
        }
    }
    save(st);
}

/// File `name` first seen (the watch started): each anchor checked
/// against it, looked for by its quote where its text is not at its place.
fn settle_anchors(st: &mut State, name: &str, text: &str) {
    let mut changed = false;
    for n in &mut st.store.notes {
        let a = &mut n.anchor;
        if a.file != name || a.quote.is_empty() {
            continue;
        }
        if text.get(a.start..a.end) == Some(a.quote.as_str()) {
            if n.detached {
                n.detached = false;
                changed = true;
            }
            continue;
        }
        match rt::find_quote(text, &a.quote, &a.before, &a.after, a.start) {
            Some((s, e)) => {
                a.start = s;
                a.end = e;
                a.line = rt::line_of(text, s);
                n.detached = false;
            }
            None => n.detached = true,
        }
        changed = true;
    }
    if changed {
        save(st);
    }
}

/// The comments on file `name`'s hunks told where the hunk changed
/// ("updated", its new diff), each moved to its hunk's id.
fn update_comments(st: &mut State, name: &str) {
    let hunks = st
        .files
        .get(name)
        .map(|t| t.hunks.clone())
        .unwrap_or_default();
    let mut changed = false;
    let at = now();
    // (a change accepted or rejected keeps its comments: a later change
    // at its place is another, as a pull request's outdated thread)
    let decided: HashSet<String> = st
        .store
        .verdicts
        .iter()
        .map(|v| v.hunk.id.clone())
        .collect();
    let st_text: HashMap<String, String> = st
        .files
        .get(name)
        .and_then(|t| t.text.clone())
        .map(|t| (name.to_owned(), t))
        .into_iter()
        .collect();
    for n in &mut st.store.notes {
        let Some(h0) = n.hunk.clone() else {
            continue;
        };
        if h0.file != name || n.detached || decided.contains(&h0.id) {
            continue;
        }
        let (s, e) = (n.anchor.start, n.anchor.end);
        // (its change where it is: by place first, alike changes elsewhere
        // share a name)
        let Some(h) = hunks
            .iter()
            .find(|h| s <= h.now.1 && h.now.0 <= e)
            .or_else(|| hunks.iter().find(|h| h.id == h0.id))
        else {
            continue;
        };
        // (the comment on the change's text as it is now)
        if (n.anchor.start, n.anchor.end) != h.now && h.now.1 > h.now.0 {
            let t = st_text.get(name).map_or("", String::as_str);
            if let Some(q) = t.get(h.now.0..h.now.1) {
                n.anchor.start = h.now.0;
                n.anchor.end = h.now.1;
                q.clone_into(&mut n.anchor.quote);
                changed = true;
            }
        }
        if h.id != h0.id {
            if h.old != h0.old || h.new != h0.new {
                n.thread.push(Post {
                    by: "agent".into(),
                    text: String::new(),
                    at: at.clone(),
                    edited: Vec::new(),
                    diff: Some(h.as_ref()),
                });
                n.updated.clone_from(&at);
            }
            n.hunk = Some(h.as_ref());
            changed = true;
        }
    }
    if changed {
        save(st);
    }
}

/// A note as the CLI and the viewer are given it: the stored note, its
/// line now, and the hunks it answered.
fn note_out(st: &State, id: &str) -> Option<Value> {
    let n = st.store.note(id)?;
    let mut v = json!(n);
    let hunks: Vec<&str> = st
        .files
        .values()
        .flat_map(|t| t.hunks.iter())
        .filter(|h| answered(n, h))
        .map(|h| h.id.as_str())
        .collect();
    v["hunks"] = json!(hunks);
    // (a comment on a change accepted or rejected since: the verdict)
    if let Some(h) = &n.hunk
        && let Some(x) = st.store.verdicts.iter().rev().find(|x| x.hunk.id == h.id)
    {
        v["verdict"] = json!({"verdict": x.verdict, "at": x.at});
    }
    Some(v)
}

/// Whether hunk `h` is one of the edits note `n`'s thread recorded.
fn answered(n: &Note, h: &HunkNow) -> bool {
    n.thread
        .iter()
        .flat_map(|p| p.edited.iter())
        .any(|e| e.file == h.file && e.start <= h.now.1 && h.now.0 <= e.end.max(e.start))
}

fn hunks_out(st: &State) -> Vec<Value> {
    let mut all: Vec<&HunkNow> = st.files.values().flat_map(|t| t.hunks.iter()).collect();
    all.sort_by(|a, b| (a.file.as_str(), a.now.0).cmp(&(b.file.as_str(), b.now.0)));
    all.into_iter()
        .map(|h| {
            let note = st
                .store
                .notes
                .iter()
                .find(|n| n.hunk.is_none() && answered(n, h))
                .map(|n| n.id.clone());
            let comments: Vec<&str> = st
                .store
                .notes
                .iter()
                .filter(|n| n.hunk.as_ref().is_some_and(|r| r.id == h.id))
                .map(|n| n.id.as_str())
                .collect();
            let text = st
                .files
                .get(&h.file)
                .and_then(|t| t.text.as_deref())
                .unwrap_or("");
            // (a line of context each side, cut at 48 characters)
            let ls = text[..h.now.0].rfind('\n').map_or(0, |i| i + 1);
            let le = text[h.now.1..]
                .find('\n')
                .map_or(text.len(), |i| h.now.1 + i);
            let clip = |s: &str, tail: bool| -> String {
                let n = s.chars().count();
                if n <= 48 {
                    return s.to_owned();
                }
                if tail {
                    format!("…{}", s.chars().skip(n - 47).collect::<String>())
                } else {
                    format!("{}…", s.chars().take(47).collect::<String>())
                }
            };
            json!({
                "id": h.id,
                "file": h.file,
                "line": h.line,
                "end_line": h.end_line,
                "base": [h.base.0, h.base.1],
                "now": [h.now.0, h.now.1],
                "parts": h.parts.iter().map(|(k, t)| json!([k, t])).collect::<Vec<_>>(),
                "before": clip(&text[ls..h.now.0], true),
                "after": clip(&text[h.now.1..le], false),
                "old": h.old,
                "new": h.new,
                "whole": h.whole,
                "note": note,
                "comments": comments,
            })
        })
        .collect()
}

/// The `review` event's state (§9.5).
fn state_json(st: &State) -> Value {
    let notes: Vec<Value> = st
        .store
        .notes
        .iter()
        .filter_map(|n| note_out(st, &n.id))
        .collect();
    json!({
        "event": "review",
        "notes": notes,
        "hunks": hunks_out(st),
        "reviews": st.store.reviews,
        "verdicts": st.store.verdicts.iter().rev().take(50).collect::<Vec<_>>(),
        "git": st.baseline.as_ref().is_some_and(Baseline::git),
        "changes": st.changes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "phitex-review-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("a folder");
        for (n, t) in files {
            let p = d.join(n);
            std::fs::create_dir_all(p.parent().expect("a folder")).expect("its folder");
            std::fs::write(p, t).expect("written");
        }
        d
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs")
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    fn facts(n: usize, files: &[&str]) -> BuildFacts {
        BuildFacts {
            n,
            ok: true,
            pages: 1,
            ms: 1.0,
            errors: Vec::new(),
            main: ("main.tex".into(), None),
            paths: files.iter().map(|s| (*s).to_owned()).collect(),
            missing: Vec::new(),
            diags_pending: false,
            diags: Some(Vec::new()),
            pdf: None,
        }
    }

    /// A hub on `dir`, its first build in having read `files` from disk.
    fn hub_on(dir: &Path, files: &[&str]) -> Hub {
        let h = Hub::default();
        h.start(dir, "main.tex", true, false);
        h.set_changes(true);
        for f in files {
            let b = std::fs::read(dir.join(f)).expect("there");
            lock(&h.st).fed.insert((*f).to_owned(), hash(&b));
        }
        h.built(facts(1, files));
        h
    }

    fn pending(h: &Hub) -> Vec<Value> {
        hunks_out(&lock(&h.st))
    }

    fn rebuilt(h: &Hub, dir: &Path, n: usize, files: &[&str]) {
        for f in files {
            if let Ok(b) = std::fs::read(dir.join(f)) {
                lock(&h.st).fed.insert((*f).to_owned(), hash(&b));
            }
        }
        h.built(facts(n, files));
    }

    const DOC: &str = "\\documentclass{article}\n\\begin{document}\nThe quick brown fox jumps over the lazy dog.\n\nA second paragraph, with words.\n\\end{document}\n";

    #[test]
    fn wait_is_exact_and_never_answers_with_an_older_build() {
        let d = project(&[("main.tex", DOC)]);
        let h = Arc::new(hub_on(&d, &["main.tex"]));
        // (nothing changed: at once)
        let v = h.wait(&json!({"timeout_ms": 5000})).expect("answered");
        assert_eq!(v["build"], 1);
        assert!(v["waited_ms"].as_f64().unwrap() < 1000.0);
        // (the agent saves: build 1 read the old text, so wait waits for 2)
        std::fs::write(d.join("main.tex"), DOC.replace("quick", "slow")).expect("saved");
        let h2 = h.clone();
        let w = std::thread::spawn(move || h2.wait(&json!({"timeout_ms": 20000})));
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !w.is_finished(),
            "answered before the build that read the save"
        );
        // (a build that read something else does not answer it)
        lock(&h.st).fed.insert("main.tex".into(), 7);
        h.built(facts(2, &["main.tex"]));
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !w.is_finished(),
            "answered by a build that read other bytes"
        );
        rebuilt(&h, &d, 3, &["main.tex"]);
        let v = w.join().expect("joined").expect("answered");
        assert_eq!(v["build"], 3);
        // (the first text back: build 1 read it, at once)
        std::fs::write(d.join("main.tex"), DOC).expect("saved");
        let v = h.wait(&json!({"timeout_ms": 5000})).expect("answered");
        assert_eq!(v["build"], 1);
        // (a timeout says so)
        std::fs::write(d.join("main.tex"), DOC.replace("lazy", "busy")).expect("saved");
        let v = h.wait(&json!({"timeout_ms": 50})).expect("answered");
        assert_eq!(v["timed_out"], true);
    }

    #[test]
    fn wait_for_warnings_and_the_pdf_waits_for_them() {
        let d = project(&[("main.tex", DOC)]);
        let h = Arc::new(hub_on(&d, &["main.tex"]));
        {
            let mut st = lock(&h.st);
            st.builds.back_mut().expect("a build").diags_pending = true;
        }
        let h2 = h.clone();
        let w = std::thread::spawn(move || {
            h2.wait(&json!({"warnings": true, "pdf": true, "timeout_ms": 20000}))
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(!w.is_finished());
        h.diagnostics(
            1,
            &[json!({"severity": "warning", "message": "w", "file": "main.tex", "line": 3})],
        );
        std::thread::sleep(Duration::from_millis(50));
        assert!(!w.is_finished(), "answered before the PDF");
        h.written(1);
        let v = w.join().expect("joined").expect("answered");
        assert_eq!(v["warnings"], 1);
    }

    #[test]
    fn notes_follow_word_edits_paragraph_inserts_and_detach() {
        let d = project(&[("main.tex", DOC)]);
        let h = hub_on(&d, &["main.tex"]);
        let s = DOC.find("brown fox").unwrap();
        let n = h
            .viewer("review_note", &json!({"kind": "comment", "text": "Is it brown?", "anchor": {"file": "main.tex", "start": s, "end": s + 9}}))
            .expect("made");
        let id = n["id"].as_str().unwrap().to_owned();
        assert_eq!(n["anchor"]["quote"], "brown fox");
        // (a word edit before it, a paragraph inserted before it)
        let t = DOC.replace("The quick", "A very quick").replace(
            "\\begin{document}\n",
            "\\begin{document}\nA new first paragraph.\n\n",
        );
        std::fs::write(d.join("main.tex"), &t).expect("saved");
        rebuilt(&h, &d, 2, &["main.tex"]);
        let st = lock(&h.st);
        let a = &st.store.note(&id).unwrap().anchor;
        assert_eq!(&t[a.start..a.end], "brown fox");
        assert_eq!(a.line, 5);
        drop(st);
        // (a word inside it changed: kept, its quote the new words)
        let t2 = t.replace("brown fox", "brown cat");
        std::fs::write(d.join("main.tex"), &t2).expect("saved");
        rebuilt(&h, &d, 3, &["main.tex"]);
        assert_eq!(
            lock(&h.st).store.note(&id).unwrap().anchor.quote,
            "brown cat"
        );
        // (its words gone: detached, never dropped)
        let t3 = t2.replace("brown cat jumps", "dog sleeps");
        std::fs::write(d.join("main.tex"), &t3).expect("saved");
        rebuilt(&h, &d, 4, &["main.tex"]);
        let st = lock(&h.st);
        assert!(st.store.note(&id).unwrap().detached);
        drop(st);
        // (kept in the file)
        let back = Store::load(&d).expect("read");
        assert!(back.note(&id).is_some_and(|n| n.detached));
    }

    #[test]
    fn notes_in_an_input_file_and_the_agents_resolve_records_its_edit() {
        let d = project(&[
            (
                "main.tex",
                "\\documentclass{article}\\begin{document}\\input{ch1}\\end{document}\n",
            ),
            ("ch1.tex", "First line.\nSecond line here.\nThird.\n"),
        ]);
        let h = hub_on(&d, &["main.tex", "ch1.tex"]);
        let n = h
            .viewer("review_note", &json!({"kind": "comment", "text": "Say more", "anchor": {"file": "ch1.tex", "start": 12, "end": 23}}))
            .expect("made");
        let id = n["id"].as_str().unwrap().to_owned();
        let listed = h.list(&json!({}));
        assert_eq!(listed["notes"][0]["anchor"]["quote"], "Second line");
        std::fs::write(
            d.join("ch1.tex"),
            "First line.\nSecond line here, and more.\nThird.\n",
        )
        .expect("saved");
        rebuilt(&h, &d, 2, &["main.tex", "ch1.tex"]);
        let r = h
            .resolve(&json!({"id": id, "text": "Done"}), "agent")
            .expect("resolved");
        assert_eq!(r["status"], "resolved");
        let e = &r["thread"][1]["edited"][0];
        assert_eq!(
            (e["file"].as_str(), e["from_line"].as_u64()),
            (Some("ch1.tex"), Some(2))
        );
        // (the hunk it made is grouped under the note)
        let p = pending(&h);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["note"], id.as_str());
    }

    #[test]
    fn with_reviewing_off_nothing_of_it_is_done() {
        let d = project(&[("main.tex", DOC)]);
        let h = Hub::default();
        h.start(&d, "main.tex", true, false);
        lock(&h.st)
            .fed
            .insert("main.tex".into(), hash(DOC.as_bytes()));
        h.built(facts(1, &["main.tex"]));
        std::fs::write(d.join("main.tex"), DOC.replace("quick", "QUICK")).expect("edited");
        rebuilt(&h, &d, 2, &["main.tex"]);
        assert_eq!(pending(&h), Vec::<Value>::new());
        assert!(lock(&h.st).baseline.is_none(), "no baseline read");
        assert!(!d.join(".phitex").exists(), "nothing written");
        let c = h.changes();
        assert_eq!(c["off"], true);
        assert!(c["how"].as_str().unwrap().contains("experimental-review"));
        let e = h
            .viewer("review_accept", &json!({"ids": "all"}))
            .unwrap_err();
        assert!(e.contains("experimental"), "{e}");
        // (notes still follow their text)
        let s0 = DOC.find("lazy").unwrap();
        h.viewer("review_note", &json!({"kind": "note", "anchor": {"file": "main.tex", "start": s0, "end": s0 + 4}, "text": "x"})).unwrap();
        std::fs::write(
            d.join("main.tex"),
            DOC.replace("quick", "QUICK")
                .replace("The QUICK", "So the QUICK"),
        )
        .expect("edited");
        rebuilt(&h, &d, 3, &["main.tex"]);
        assert_eq!(
            h.viewer("review_state", &json!({})).unwrap()["notes"][0]["anchor"]["quote"],
            "lazy"
        );
        // (turned on: the text as it is now is the accepted one, then edits wait)
        h.set_changes(true);
        assert_eq!(pending(&h), Vec::<Value>::new());
        std::fs::write(
            d.join("main.tex"),
            DOC.replace("quick", "QUICK")
                .replace("The QUICK", "So the QUICK")
                .replace("dog", "cat"),
        )
        .expect("edited");
        rebuilt(&h, &d, 4, &["main.tex"]);
        assert_eq!(pending(&h).len(), 1);
    }

    #[test]
    fn accept_and_reject_against_a_baseline_folder() {
        let d = project(&[("main.tex", DOC)]);
        let h = hub_on(&d, &["main.tex"]);
        assert_eq!(
            pending(&h),
            Vec::<Value>::new(),
            "the first text seen is the baseline"
        );
        // (nothing to review: nothing written into the project)
        rebuilt(&h, &d, 2, &["main.tex"]);
        assert!(
            !d.join(".phitex").exists(),
            "a watch with nothing to review wrote .phitex"
        );
        let t = DOC.replace("quick", "QUICK").replace("second", "SECOND");
        std::fs::write(d.join("main.tex"), &t).expect("saved");
        rebuilt(&h, &d, 3, &["main.tex"]);
        let p = pending(&h);
        assert_eq!(p.len(), 2, "{p:?}");
        // (a change waits: the text as found kept in the folder)
        assert!(d.join(".phitex/baseline/main.tex").exists());
        // (accept the first: the baseline has it, one left)
        h.viewer("review_accept", &json!({"ids": [p[0]["id"]]}))
            .expect("accepted");
        assert!(
            std::fs::read_to_string(d.join(".phitex/baseline/main.tex"))
                .unwrap()
                .contains("QUICK")
        );
        let p = pending(&h);
        assert_eq!(p.len(), 1);
        // (reject the second: the disk has the old words back)
        h.viewer("review_reject", &json!({"ids": [p[0]["id"]]}))
            .expect("rejected");
        let disk = std::fs::read_to_string(d.join("main.tex")).unwrap();
        assert!(disk.contains("QUICK") && disk.contains("second"));
        assert_eq!(pending(&h), Vec::<Value>::new());
        // (the agent is told what was rejected, once)
        let c = h.changes();
        assert_eq!(c["verdicts"].as_array().unwrap().len(), 2);
        assert_eq!(c["verdicts"][1]["verdict"], "rejected");
        assert_eq!(c["verdicts"][1]["hunk"]["new"], "SECOND");
        assert_eq!(h.changes()["verdicts"], json!([]));
        // (a stale id refused)
        let e = h
            .viewer("review_accept", &json!({"ids": ["h00000000"]}))
            .unwrap_err();
        assert!(e.contains("edited again"), "{e}");
    }

    #[test]
    fn the_agent_edits_a_hunk_again_after_a_partial_accept() {
        let d = project(&[("main.tex", DOC)]);
        let h = hub_on(&d, &["main.tex"]);
        let v1 = DOC
            .replace("lazy dog", "sleepy dog")
            .replace("words.", "many words.");
        std::fs::write(d.join("main.tex"), &v1).expect("saved");
        rebuilt(&h, &d, 2, &["main.tex"]);
        let p = pending(&h);
        assert_eq!(p.len(), 2);
        h.viewer("review_accept", &json!({"ids": [p[0]["id"]]}))
            .expect("accepted");
        // (a comment on the other, then the agent changes it again)
        let c = h
            .viewer(
                "review_comment",
                &json!({"hunk": p[1]["id"], "text": "Too vague"}),
            )
            .expect("commented");
        let cid = c["id"].as_str().unwrap().to_owned();
        let v2 = v1
            .replace("sleepy dog", "drowsy dog")
            .replace("many words.", "a great many words.");
        std::fs::write(d.join("main.tex"), &v2).expect("saved");
        rebuilt(&h, &d, 3, &["main.tex"]);
        let p = pending(&h);
        // (the accepted one's change is against sleepy now; the other against the baseline still)
        let olds: Vec<&str> = p.iter().map(|x| x["old"].as_str().unwrap()).collect();
        assert!(olds.contains(&"sleepy"), "{p:?}");
        let st = lock(&h.st);
        let n = st.store.note(&cid).unwrap();
        let last = n.thread.last().unwrap();
        assert!(
            last.diff
                .as_ref()
                .is_some_and(|d| d.new.contains("great many")),
            "{:?}",
            n.thread
        );
        assert!(n.hunk.as_ref().unwrap().new.contains("great many"));
    }

    #[test]
    fn a_comment_stays_on_its_change_among_changes_alike() {
        let doc = "Item one is described. Item two is described. Item three is described.\n";
        let d = project(&[("main.tex", doc)]);
        let h = hub_on(&d, &["main.tex"]);
        let v1 = doc.replace("described", "shown");
        std::fs::write(d.join("main.tex"), &v1).expect("saved");
        rebuilt(&h, &d, 2, &["main.tex"]);
        let p = pending(&h);
        assert_eq!(p.len(), 3, "{p:?}");
        // (on the second of three alike)
        let c = h
            .viewer(
                "review_comment",
                &json!({"hunk": p[1]["id"], "text": "Say what is shown"}),
            )
            .expect("commented");
        let cid = c["id"].as_str().unwrap().to_owned();
        let v2 = v1.replace("two is shown", "two is drawn in a figure");
        std::fs::write(d.join("main.tex"), &v2).expect("saved");
        rebuilt(&h, &d, 3, &["main.tex"]);
        let st = lock(&h.st);
        let n = st.store.note(&cid).unwrap();
        assert_eq!(n.anchor.quote, "drawn in a figure", "{:?}", n.anchor);
        assert!(
            n.thread
                .last()
                .unwrap()
                .diff
                .as_ref()
                .is_some_and(|d| d.new == "drawn in a figure")
        );
        // (and its change named so after: the other two alike untouched)
        let hs = hunks_out(&st);
        let mine = hs.iter().find(|x| x["new"] == "drawn in a figure").unwrap();
        assert!(
            mine["comments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x == cid.as_str())
        );
        assert!(
            hs.iter()
                .filter(|x| x["new"] == "shown")
                .all(|x| x["comments"].as_array().unwrap().is_empty())
        );
    }

    #[test]
    fn the_folder_made_in_a_git_clone_is_excluded_there_once() {
        let d = project(&[]);
        let run = |a: &[&str]| {
            std::process::Command::new("git")
                .args(a)
                .current_dir(&d)
                .output()
                .expect("git runs")
        };
        run(&["init", "-q"]);
        let said = make_dir(&d).expect("made");
        assert!(
            said.as_deref().is_some_and(|s| s.contains("info/exclude")),
            "{said:?}"
        );
        let ex = std::fs::read_to_string(d.join(".git/info/exclude")).expect("the exclude file");
        assert_eq!(ex.matches(".phitex/").count(), 1, "{ex}");
        std::fs::remove_dir_all(d.join(".phitex")).expect("removed");
        assert_eq!(make_dir(&d).expect("made again"), None);
        let ex = std::fs::read_to_string(d.join(".git/info/exclude")).expect("the exclude file");
        assert_eq!(ex.matches(".phitex/").count(), 1, "{ex}");
        std::fs::write(d.join("a.tex"), "x").expect("a file");
        let st = String::from_utf8_lossy(
            &run(&["status", "--porcelain", "--untracked-files=all"]).stdout,
        )
        .into_owned();
        assert!(!st.contains(".phitex"), "{st}");
        // (outside git: made, nothing said)
        let plain = project(&[]);
        assert_eq!(make_dir(&plain).expect("made"), None);
        assert!(plain.join(".phitex").is_dir());
    }

    #[test]
    fn git_index_is_the_baseline() {
        let d = project(&[("main.tex", DOC)]);
        git(&d, &["init", "-q"]);
        git(&d, &["add", "main.tex"]);
        let h = hub_on(&d, &["main.tex"]);
        assert!(h.changes()["git"].as_bool().unwrap());
        let t = DOC.replace("quick", "QUICK").replace("second", "SECOND");
        std::fs::write(d.join("main.tex"), &t).expect("saved");
        rebuilt(&h, &d, 2, &["main.tex"]);
        let p = pending(&h);
        assert_eq!(p.len(), 2);
        h.viewer("review_accept", &json!({"ids": [p[0]["id"]]}))
            .expect("staged");
        let staged = Command::new("git")
            .args(["show", ":main.tex"])
            .current_dir(&d)
            .output()
            .unwrap()
            .stdout;
        let staged = String::from_utf8(staged).unwrap();
        assert!(
            staged.contains("QUICK") && staged.contains("second"),
            "{staged}"
        );
        h.viewer("review_reject", &json!({"ids": "all"}))
            .expect("rejected");
        let disk = std::fs::read_to_string(d.join("main.tex")).unwrap();
        assert_eq!(disk, staged);
    }

    #[test]
    fn a_deleted_and_a_renamed_file() {
        let d = project(&[("main.tex", DOC), ("ch1.tex", "Chapter one.\n")]);
        let h = hub_on(&d, &["main.tex", "ch1.tex"]);
        // (renamed: the old one gone, the new one added)
        std::fs::rename(d.join("ch1.tex"), d.join("intro.tex")).expect("renamed");
        rebuilt(&h, &d, 2, &["main.tex", "intro.tex"]);
        let p = pending(&h);
        let kinds: Vec<(&str, &str)> = p
            .iter()
            .map(|x| {
                (
                    x["file"].as_str().unwrap(),
                    x["whole"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert!(kinds.contains(&("ch1.tex", "deleted")), "{kinds:?}");
        // (a file that came after the first build: added, the agent's)
        assert!(kinds.contains(&("intro.tex", "added")), "{kinds:?}");
        // (reject the deletion: written back)
        let del = p.iter().find(|x| x["file"] == "ch1.tex").unwrap()["id"].clone();
        h.viewer("review_reject", &json!({"ids": [del]}))
            .expect("rejected");
        assert_eq!(
            std::fs::read_to_string(d.join("ch1.tex")).unwrap(),
            "Chapter one.\n"
        );
    }

    #[test]
    fn the_users_typing_in_an_editor_is_accepted_and_a_reject_with_unsaved_text_refused() {
        let d = project(&[("main.tex", DOC)]);
        let h = hub_on(&d, &["main.tex"]);
        // (the agent's change on disk: pending)
        let t = DOC.replace("lazy", "LAZY");
        std::fs::write(d.join("main.tex"), &t).expect("saved");
        rebuilt(&h, &d, 2, &["main.tex"]);
        assert_eq!(pending(&h).len(), 1);
        // (the editor opens it, then the user types elsewhere: accepted)
        let buf = Arc::new(Mutex::new(Some(t.clone())));
        let b2 = buf.clone();
        h.on_editors(Editors {
            buffer: Box::new(move |f| (f == "main.tex").then(|| lock(&b2).clone()).flatten()),
            replace: Box::new(|_, _| true),
        });
        h.user_edit("main.tex", None, &t, true);
        let at = t.find("second").unwrap();
        let typed = format!("{}SECOND{}", &t[..at], &t[at + 6..]);
        h.user_edit("main.tex", Some((at, at + 6)), "SECOND", false);
        *lock(&buf) = Some(typed.clone());
        rebuilt(&h, &d, 3, &["main.tex"]);
        let p = pending(&h);
        assert_eq!(
            p.len(),
            1,
            "the user's own typing is no pending change: {p:?}"
        );
        assert_eq!(p[0]["new"], "LAZY");
        // (unsaved: a reject is refused)
        let e = h
            .viewer("review_reject", &json!({"ids": [p[0]["id"]]}))
            .unwrap_err();
        assert!(e.contains("unsaved"), "{e}");
    }
}
