//! `phitex watch DIR`: a folder browsed from one viewer address. Its
//! index page is a file manager of the folder: its folders and files,
//! one folder at a time, never above the folder watched; a `.tex` file
//! picked there opens in a tab of its own and builds on every save.
//!
//! - **The listing.** Every file and folder, read when the page asks for
//!   a folder and again when the folder changes (one recursive watcher on
//!   the whole folder). Hidden ones (`.git`, `.cache`, …) and PhiTeX's
//!   cache are left out; so are links that lead out of the folder. No
//!   file is read to decide what is listed.
//! - **The bounds.** Every path the browser names is checked by
//!   [`inside`]: relative, no `..`, `.` or hidden name, inside the folder
//!   once its links are followed. A folder above, an absolute path or a
//!   link out are refused alike.
//! - **A session a document.** A document opened (`d/ID/`) is a `phitex
//!   watch FILE` of its own, started then and not before: a child
//!   process run where that command would be run ([`session_dir`]: the
//!   nearest folder from the file's up to the watched one holding its
//!   settings, `phitex.toml` or a `.latexmkrc`, else the watched one), so
//!   its settings apply as they do there. A process, not a thread: TeX
//!   names its files from the directory it runs in, and a document's
//!   memory is given back whole when its session ends. Its messages come
//!   here as JSON lines ([`crate::ui::RELAY`]), its keys go to it as
//!   lines.
//! - **One address.** One server, one port, one token: the index, and
//!   each document's viewer under `d/ID/`, its page served here and the
//!   rest passed through to its session's own server on 127.0.0.1 (its
//!   port and token known only here). The viewer asks for everything by
//!   relative address, so it is the single-file watch's page unchanged;
//!   it finds the folder's file manager at `../../ws` for its Files panel.
//! - **Lifetime, no timers.** A document builds on a change while a tab
//!   shows it. Its last tab closed, it stops building (`pause`: changes
//!   kept) and stays warm: back, it builds what changed meanwhile, an
//!   edit's build. The sessions kept together may use [`Shared::budget`]
//!   of memory; past it, the least recently viewed of those no tab shows
//!   end. Opened again, an ended one starts from its saved graph, or a
//!   clean build.
//! - **Threads.** The sessions build one at a time, each with every
//!   thread the watch was given ([`crate::watch::TURNS`]): the cores the
//!   user gave are never oversubscribed. A save in one document while
//!   another builds waits, and says so.
//! - **The terminal.** The index's address, then a status line an open
//!   document; the keys act on every open document, or on the one chosen
//!   with `1`-`9` (`a`: all again).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::UNIX_EPOCH;

use notify::{RecursiveMode, Watcher as _};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

use crate::keys::{self, Key};
use crate::modern::{Options, Ui};
use crate::ws;

/// The index's page and script (`scripts/viewer-bundle.sh`).
const INDEX: &[u8] = include_bytes!("../viewer/dir.html");
const SCRIPT: &[u8] = include_bytes!("../viewer/dir.js");

/// The keys, for `h`.
pub const HELP: &[(&str, &str)] = &[
    (
        "1-9",
        "the keys below go to that document (its status line's number)",
    ),
    ("a, 0", "the keys below go to every open document again"),
    ("r, Enter", "build again now"),
    ("p", "show the last build's errors again"),
    (
        "w",
        "the warnings, now and after every build (again: hide them)",
    ),
    (
        "s",
        "how the build went, now and after every build (again: hide it)",
    ),
    (
        "o",
        "open the folder in the browser (or the chosen document)",
    ),
    ("c", "clear the screen"),
    ("h, ?", "these keys"),
    ("q, Ctrl-C", "quit, every document's session with it"),
];

/// At most this many files are listed for quick open (a home folder
/// watched by mistake is not walked for ever).
const FIND_MAX: usize = 20_000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------
// The folder: its bounds, its listing.

/// The folder watched, and what is never listed in it.
pub struct Root {
    /// Canonical.
    pub path: PathBuf,
    /// PhiTeX's cache, canonical, when it is inside the folder.
    cache: Option<PathBuf>,
}

impl Root {
    pub fn new(path: PathBuf) -> Root {
        let cache = crate::cache::Cache::open(None)
            .ok()
            .and_then(|c| c.dir.canonicalize().ok())
            .filter(|c| c.starts_with(&path));
        Root { path, cache }
    }

    /// Whether canonical path `c` is the watch's to show: inside the
    /// folder, not in the cache.
    fn shows(&self, c: &Path) -> bool {
        c.starts_with(&self.path) && self.cache.as_ref().is_none_or(|k| !c.starts_with(k))
    }
}

/// Whether `name` is hidden (`.git`, `.latexmkrc`): never listed, never
/// served.
fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// The canonical path of `rel` (from the root, `/` between names, `""`
/// the root itself), when the watch may show it: relative, no empty,
/// `.`, `..` or hidden name, no `\` or NUL, inside the root once every
/// link is followed and not in the cache. The one gate every path the
/// browser names goes through.
pub fn inside(root: &Root, rel: &str) -> Result<PathBuf, String> {
    let refused = || format!("{rel} is not in this folder");
    if rel.is_empty() {
        return Ok(root.path.clone());
    }
    if rel.starts_with('/')
        || rel.contains('\\')
        || rel.contains('\0')
        || (cfg!(windows) && rel.contains(':'))
        || rel
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".." || hidden(c))
    {
        return Err(refused());
    }
    let c = root.path.join(rel).canonicalize().map_err(|_| refused())?;
    if !root.shows(&c) {
        return Err(refused());
    }
    Ok(c)
}

/// What a file is, for the file manager: a document (`.tex`, opened in
/// the viewer), a PDF or a picture (opened as it is in a tab), or
/// another file (shown, not opened).
fn kind_of(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .map(|x| x.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "tex" => "tex",
        "pdf" => "pdf",
        "png" | "jpg" | "jpeg" | "gif" | "webp" => "image",
        _ => "other",
    }
}

/// The media type a raw file is served with (`kind_of`'s pdf and image).
fn media_type(name: &str) -> Option<&'static str> {
    let ext = Path::new(name)
        .extension()
        .map(|x| x.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    Some(match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// One entry of a folder's listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    /// Its modification time, in milliseconds since 1970.
    pub mtime: Option<u64>,
    pub size: u64,
}

/// Folder `rel`'s entries (checked by [`inside`]), by name: hidden ones,
/// the cache and links that lead out of the root left out.
pub fn list(root: &Root, rel: &str) -> Result<Vec<Entry>, String> {
    let dir = inside(root, rel)?;
    let rd = std::fs::read_dir(&dir).map_err(|_| format!("{rel} is not a folder of this watch"))?;
    let mut out = Vec::new();
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        if hidden(&name) {
            continue;
        }
        let path = e.path();
        let link = e.file_type().is_ok_and(|t| t.is_symlink());
        // (a link: listed where it leads inside the root, else not at all)
        if link && !path.canonicalize().is_ok_and(|c| root.shows(&c)) {
            continue;
        }
        if !link && root.cache.as_ref().is_some_and(|k| path == *k) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_dir() && !meta.is_file() {
            continue;
        }
        out.push(Entry {
            name,
            dir: meta.is_dir(),
            mtime: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|d| u64::try_from(d.as_millis()).ok()),
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Every `.tex` file below the root (from it), for quick open: the
/// listing's rules, each folder once (links followed inside the root),
/// at most [`FIND_MAX`].
pub fn find_tex(root: &Root) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::from([root.path.clone()]);
    // (breadth first, in the listing's order: a folder reached by a link
    // too is listed under its own name)
    let mut todo = std::collections::VecDeque::from([String::new()]);
    while let Some(rel) = todo.pop_front() {
        let Ok(entries) = list(root, &rel) else {
            continue;
        };
        for e in entries {
            let child = if rel.is_empty() {
                e.name.clone()
            } else {
                format!("{rel}/{}", e.name)
            };
            if e.dir {
                if let Ok(c) = root.path.join(&child).canonicalize()
                    && seen.insert(c)
                {
                    todo.push_back(child);
                }
            } else if kind_of(&e.name) == "tex" {
                out.push(child);
                if out.len() >= FIND_MAX {
                    return out;
                }
            }
        }
    }
    out.sort();
    out
}

/// The folder a document's session runs in: the nearest from its file's
/// up to the root that holds its settings (`phitex.toml`, `.latexmkrc`,
/// `latexmkrc`), else the root: where `phitex watch` would be run on it.
pub fn session_dir(root: &Path, rel: &str) -> PathBuf {
    let file = root.join(rel);
    let mut d = file.parent();
    while let Some(x) = d {
        if [crate::config::FILE, ".latexmkrc", "latexmkrc"]
            .iter()
            .any(|f| x.join(f).is_file())
        {
            return x.to_path_buf();
        }
        if x == root {
            break;
        }
        d = x.parent();
    }
    root.to_path_buf()
}

/// A document's address under the index (`d/ID/`): from its path, so a
/// tab left open finds it again in the next watch.
pub fn doc_id(rel: &str) -> String {
    let h = crate::tex::Fnv::new().add(rel.as_bytes()).0;
    format!("{:012x}", h & 0xffff_ffff_ffff)
}

/// The folder `rel` is in (`""`: the root).
fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |x| x.0)
}

/// The document opened for `rel`: the file its `% !TEX root` names
/// (from its folder, a `.tex` file inside the root), else itself; and
/// why another one, or why not the one named.
pub fn root_of(root: &Root, rel: &str) -> (String, Option<String>) {
    let Ok(file) = inside(root, rel) else {
        return (rel.to_owned(), None);
    };
    let mut head = Vec::new();
    if std::fs::File::open(&file)
        .and_then(|f| f.take(8192).read_to_end(&mut head))
        .is_err()
    {
        return (rel.to_owned(), None);
    }
    let (_, named) = crate::config::magic_comments(&String::from_utf8_lossy(&head));
    let Some(named) = named.filter(|n| !n.is_empty()) else {
        return (rel.to_owned(), None);
    };
    let here = parent(rel);
    // (the name from the file's folder, `..` taken in turn, never above the root)
    let mut parts: Vec<&str> = here.split('/').filter(|c| !c.is_empty()).collect();
    let mut ok = !named.starts_with('/') && !named.contains('\\');
    for c in named.split('/') {
        match c {
            "" | "." => {}
            ".." => ok &= parts.pop().is_some(),
            c => parts.push(c),
        }
    }
    let target = parts.join("/");
    let target = if Path::new(&target).extension().is_none() {
        format!("{target}.tex")
    } else {
        target
    };
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if ok && kind_of(&target) == "tex" && inside(root, &target).is_ok_and(|p| p.is_file()) {
        if target == rel {
            return (target, None);
        }
        return (
            target.clone(),
            Some(format!(
                "Opened {target}: {name} names it as its root (% !TEX root)"
            )),
        );
    }
    (
        rel.to_owned(),
        Some(format!(
            "{name} names {named} as its root (% !TEX root), not a .tex file in this folder: opened {name} itself"
        )),
    )
}

// ---------------------------------------------------------------------
// The sessions.

/// A document's session, a `phitex watch` of its own.
struct Running {
    stdin: Option<ChildStdin>,
    pid: u32,
    epoch: u64,
    /// Its viewer's address (once it said it).
    url: Option<String>,
    /// Ended by the watch (quit, the memory budget), not on its own.
    stopping: bool,
}

/// A document opened in this run.
struct Doc {
    /// The last build: "none" (not built), "building", "ok", "errors".
    build: &'static str,
    /// Its build waits for another document's.
    queued: bool,
    errors: usize,
    pages: usize,
    /// Why its session ended or failed.
    message: Option<String>,
    run: Option<Running>,
    /// Building on a change (a tab shows it, or it was just opened);
    /// else paused, kept warm.
    on: bool,
    /// Its session ended on its own: its tabs are not let in again until
    /// it is opened again (a session that fails at its start would be
    /// started again by its tab's every try).
    held: bool,
    /// The tabs showing it (their WebSockets, numbered).
    tabs: Vec<(u64, TcpStream)>,
    /// When a tab last showed it (the watch's own clock: [`State::clock`]).
    viewed: u64,
    /// Its session's status line and mark, and what its build is doing.
    status: String,
    ok: Option<bool>,
    busy: String,
    last_error: Option<String>,
}

impl Doc {
    fn new() -> Doc {
        Doc {
            build: "none",
            queued: false,
            errors: 0,
            pages: 0,
            message: None,
            run: None,
            on: false,
            held: false,
            tabs: Vec::new(),
            viewed: 0,
            status: String::new(),
            ok: None,
            busy: String::new(),
            last_error: None,
        }
    }

    /// A line to its session (a key, `pause`, `resume`, `quit`).
    fn send(&mut self, line: &str) {
        if let Some(i) = self.run.as_mut().and_then(|r| r.stdin.as_mut()) {
            let _ = writeln!(i, "{line}");
            let _ = i.flush();
        }
    }

    /// Its session asked to end, as `q` (its records stored, the exact
    /// PDF written), its standard input closed after.
    fn stop(&mut self) {
        self.send("quit");
        if let Some(r) = self.run.as_mut() {
            r.stopping = true;
            r.stdin = None;
        }
    }

    /// Its tabs' connections cut.
    fn cut(&mut self) {
        for (_, s) in self.tabs.drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    fn json(&self) -> Value {
        json!({
            "build": if self.queued { "waiting" } else { self.build },
            "errors": self.errors,
            "pages": self.pages,
            "tabs": self.tabs.len(),
            "live": self.run.is_some(),
            "message": self.message,
        })
    }
}

#[derive(Default)]
struct State {
    /// The documents opened this run, by path from the root.
    docs: BTreeMap<String, Doc>,
    /// Every document address handed out or asked for: its path.
    ids: HashMap<String, String>,
    /// Changed at every change of a document's state.
    version: u64,
    epoch: u64,
    /// The watch's clock for [`Doc::viewed`]: one tick an event.
    clock: u64,
    /// The watch is quitting: no session starts any more (a tab cut
    /// joins again at once, and would start its document's anew).
    quitting: bool,
    /// Quick open's list, and the folder's version it was made at.
    found: Option<(u64, Vec<String>)>,
}

/// What wakes the watch's loop.
enum Ev {
    Changed,
    /// A key, or a line on a pipe (its key, and the document it names).
    Key(Key, Option<String>),
}

pub struct Shared {
    root: Root,
    url: String,
    token: String,
    st: Mutex<State>,
    cv: Condvar,
    /// The folder's files changed this many times (the listings sent
    /// again, quick open's list made again).
    files: AtomicU64,
    /// A session's command line, before its file.
    args: Vec<String>,
    turns: PathBuf,
    tx: Mutex<mpsc::Sender<Ev>>,
    ui: Ui,
    /// The memory the sessions may keep together, in bytes.
    budget: u64,
}

impl Shared {
    fn changed(&self, st: &mut State) {
        st.version += 1;
        self.cv.notify_all();
        let _ = lock(&self.tx).send(Ev::Changed);
    }

    /// The path document address `id` stands for: one handed out, else
    /// found among the folder's `.tex` files (a tab left open from the
    /// last watch).
    fn rel_of(&self, id: &str) -> Option<String> {
        if let Some(r) = lock(&self.st).ids.get(id) {
            return Some(r.clone());
        }
        let all = self.found();
        let mut st = lock(&self.st);
        for r in all {
            st.ids.insert(doc_id(&r), r);
        }
        st.ids.get(id).cloned()
    }

    /// Quick open's list: every `.tex` file, made again when the folder
    /// changed.
    fn found(&self) -> Vec<String> {
        let v = self.files.load(Ordering::Acquire);
        if let Some((at, l)) = &lock(&self.st).found
            && *at == v
        {
            return l.clone();
        }
        let l = find_tex(&self.root);
        lock(&self.st).found = Some((v, l.clone()));
        l
    }
}

/// The command line of a session: the watch's (`args`, after the
/// program) without its folder (`dir`), `--open`, `--json`, `--port` and
/// `--memory`: the session's viewer is the index's, its events come here.
pub fn session_args(args: &[String], dir: &str) -> Vec<String> {
    // (the folder: the word whose removal leaves no file, `-o out out`'s second)
    let at = (0..args.len()).rev().find(|&i| {
        args[i] == dir && {
            let mut t = args.to_vec();
            t.remove(i);
            crate::modern::parse_options(&t).is_ok_and(|o| o.file.is_none())
        }
    });
    let mut v = Vec::new();
    let mut k = 0;
    while k < args.len() {
        let a = &args[k];
        if Some(k) != at {
            match a.as_str() {
                "--open" | "--json" => {}
                "--port" | "--memory" => k += 1,
                _ if a.starts_with("--port=") || a.starts_with("--memory=") => {}
                _ => v.push(a.clone()),
            }
        }
        k += 1;
    }
    v
}

/// The resident memory of process `pid`, in bytes (Linux; elsewhere none,
/// and the budget never ends a session).
fn rss_of(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
        return Some(pages * rustix::param::page_size() as u64);
    }
    #[allow(unreachable_code)]
    {
        let _ = pid;
        None
    }
}

/// The default memory budget: a quarter of the machine's, 4 GB where it
/// can't be read.
pub fn default_budget() -> u64 {
    #[cfg(target_os = "linux")]
    if let Ok(m) = std::fs::read_to_string("/proc/meminfo")
        && let Some(kb) = m
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))
            .and_then(|r| r.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
    {
        return kb * 1024 / 4;
    }
    4_000_000_000
}

/// The sessions over the budget ended: those no tab shows, the least
/// recently viewed first, until the rest fit.
fn enforce_budget(sh: &Shared, st: &mut State) {
    let mut total = 0u64;
    let mut idle: Vec<(u64, String, u64)> = Vec::new();
    for (r, d) in &st.docs {
        let Some(run) = d.run.as_ref().filter(|r| !r.stopping) else {
            continue;
        };
        let m = rss_of(run.pid).unwrap_or(0);
        total += m;
        if !d.on && d.tabs.is_empty() {
            idle.push((d.viewed, r.clone(), m));
        }
    }
    if total <= sh.budget {
        return;
    }
    idle.sort();
    let mut ended = false;
    for (_, r, m) in idle {
        if total <= sh.budget {
            break;
        }
        if let Some(d) = st.docs.get_mut(&r) {
            d.stop();
            d.message = Some("ended to free memory: opened again, it builds again".to_owned());
            sh.ui.line(&sh.ui.st.dim(&format!(
                "{r}: ended, the open documents' memory past {} (opened again, it builds again)",
                bytesize::ByteSize::b(sh.budget).display().si()
            )));
            sh.ui
                .event(&json!({"event": "session", "doc": r, "ended": "memory"}));
            total = total.saturating_sub(m);
            ended = true;
        }
    }
    if ended {
        sh.changed(st);
    }
}

/// Document `rel`'s session started (generation `epoch`): its messages read
/// on threads of their own, into the state.
fn spawn(sh: &Arc<Shared>, rel: &str, epoch: u64) -> std::io::Result<Running> {
    let file = sh.root.path.join(rel);
    let dir = session_dir(&sh.root.path, rel);
    let mut arg = file
        .strip_prefix(&dir)
        .unwrap_or(&file)
        .to_string_lossy()
        .into_owned();
    if arg.starts_with('-') {
        arg = format!("./{arg}");
    }
    let mut child = Command::new(std::env::current_exe()?)
        .args(&sh.args)
        .arg(&arg)
        .args(["--port", "0"])
        .env(crate::ui::RELAY, "1")
        .env(crate::watch::TURNS, &sh.turns)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let stdin = child.stdin.take();
    let out = child.stdout.take();
    let err = child.stderr.take();
    if let Some(err) = err {
        let (sh, rel) = (sh.clone(), rel.to_owned());
        std::thread::spawn(move || {
            for l in BufReader::new(err).lines().map_while(Result::ok) {
                sh.ui
                    .line(&format!("{} {l}", sh.ui.st.dim(&format!("{rel}:"))));
            }
        });
    }
    {
        let (sh, rel) = (sh.clone(), rel.to_owned());
        std::thread::spawn(move || {
            if let Some(out) = out {
                for l in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Ok(v) = serde_json::from_str::<Value>(&l) {
                        told(&sh, &rel, epoch, &v);
                    }
                }
            }
            // (its output closed: it ends)
            let code = child.wait().ok().and_then(|s| s.code());
            ended(&sh, &rel, epoch, code);
        });
    }
    Ok(Running {
        stdin,
        pid,
        epoch,
        url: None,
        stopping: false,
    })
}

/// What session `epoch` of `rel` said (a relayed message).
fn told(sh: &Shared, rel: &str, epoch: u64, v: &Value) {
    let ui = &sh.ui;
    let text = v["text"].as_str().unwrap_or("");
    let mut st = lock(&sh.st);
    let Some(d) = st.docs.get_mut(rel) else {
        return;
    };
    if d.run.as_ref().is_none_or(|r| r.epoch != epoch) {
        return;
    }
    let mut measure = false;
    match v["kind"].as_str().unwrap_or("") {
        "event" => {
            let e = &v["event"];
            match e["event"].as_str().unwrap_or("") {
                "viewer" => {
                    if let Some(r) = d.run.as_mut() {
                        r.url = e["url"].as_str().map(str::to_owned);
                    }
                }
                "started" => d.build = "building",
                "queued" => d.queued = e["on"] == true,
                "build" => {
                    d.build = if e["ok"] == true { "ok" } else { "errors" };
                    d.errors = e["errors"].as_array().map_or(0, Vec::len);
                    d.pages = usize::try_from(e["pages"].as_u64().unwrap_or(0)).unwrap_or(0);
                    d.message = None;
                    d.queued = false;
                    measure = true;
                }
                "failed" => {
                    d.build = "errors";
                    d.errors = d.errors.max(1);
                    d.message = e["message"].as_str().map(str::to_owned);
                    d.queued = false;
                    measure = true;
                }
                _ => {}
            }
            // (`--json`: every session's events, each with its document)
            let mut e = e.clone();
            if let Some(o) = e.as_object_mut() {
                o.insert("doc".into(), json!(rel));
            }
            ui.event(&e);
        }
        "status" => {
            text.clone_into(&mut d.status);
            d.ok = v["ok"].as_bool();
        }
        "busy" => text.clone_into(&mut d.busy),
        "error" => {
            d.last_error = Some(text.to_owned());
            ui.error(&format!("{rel}: {text}"));
        }
        "warning" => ui.warning(&format!("{rel}: {text}")),
        "note" => ui.note(&format!("{rel}: {text}")),
        "line" => ui.line(&format!("{} {text}", ui.st.cyan(&format!("{rel}:")))),
        _ => return,
    }
    sh.changed(&mut st);
    // (a build in: its memory with it, the budget checked)
    if measure {
        enforce_budget(sh, &mut st);
    }
}

/// Session `epoch` of `rel` ended (`code`: its exit status).
fn ended(sh: &Shared, rel: &str, epoch: u64, code: Option<i32>) {
    let mut st = lock(&sh.st);
    let Some(d) = st.docs.get_mut(rel) else {
        return;
    };
    let Some(r) = d.run.take_if(|r| r.epoch == epoch) else {
        return;
    };
    d.on = false;
    d.queued = false;
    d.busy.clear();
    d.cut();
    if !r.stopping {
        // (on its own: said, and not started again by a tab still asking
        // until it is opened again)
        let why = d.last_error.clone().unwrap_or_else(|| match code {
            Some(c) => format!("its session ended (exit status {c})"),
            None => "its session ended".to_owned(),
        });
        sh.ui.warning(&format!("{rel}: {why}"));
        d.message = Some(why);
        d.held = true;
        if d.build == "building" || d.build == "none" {
            d.build = "errors";
            d.errors = d.errors.max(1);
        }
    }
    sh.ui
        .event(&json!({"event": "session", "doc": rel, "running": false}));
    sh.changed(&mut st);
}

/// Document `rel` shown: its session started, or resumed if kept.
/// `asked`: opened (its page loaded); else a tab joining, refused while
/// its session is held (ended on its own).
fn connect(sh: &Arc<Shared>, rel: &str, asked: bool) -> Result<(), String> {
    let mut st = lock(&sh.st);
    st.epoch += 1;
    st.clock += 1;
    let (epoch, clock) = (st.epoch, st.clock);
    if st.quitting {
        return Err("phitex watch is quitting".to_owned());
    }
    let d = st.docs.entry(rel.to_owned()).or_insert_with(Doc::new);
    d.viewed = clock;
    if asked {
        d.held = false;
    }
    if d.held {
        return Err(format!("{rel}: its session ended"));
    }
    if d.on && d.run.is_some() {
        return Ok(());
    }
    match d.run.as_ref() {
        Some(r) if !r.stopping => {
            d.send("resume");
            d.on = true;
        }
        _ => {
            let r = spawn(sh, rel, epoch).map_err(|e| format!("{rel}: its session: {e}"))?;
            d.run = Some(r);
            d.on = true;
            d.message = None;
            d.last_error = None;
        }
    }
    sh.ui
        .event(&json!({"event": "session", "doc": rel, "connected": true}));
    sh.changed(&mut st);
    Ok(())
}

/// Document `rel`'s last tab gone: no builds, its session kept warm while
/// the budget allows.
fn disconnect(sh: &Shared, st: &mut State, rel: &str) {
    st.clock += 1;
    let clock = st.clock;
    let Some(d) = st.docs.get_mut(rel) else {
        return;
    };
    d.viewed = clock;
    if d.on && d.run.as_ref().is_some_and(|r| !r.stopping) {
        d.send("pause");
        d.on = false;
        sh.ui
            .event(&json!({"event": "session", "doc": rel, "connected": false}));
    }
    sh.changed(st);
    enforce_budget(sh, st);
}

// ---------------------------------------------------------------------
// The server: the index, raw files, and each document's viewer.

/// `%XX` escapes decoded (a path the browser sent); none if not UTF-8.
fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut k = 0;
    while k < b.len() {
        if b[k] == b'%' {
            let h = std::str::from_utf8(b.get(k + 1..k + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            k += 3;
        } else {
            out.push(b[k]);
            k += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A path for an address: each name's bytes but the safe ones escaped.
fn escape(rel: &str) -> String {
    let mut s = String::new();
    for b in rel.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(char::from(b));
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn redirect(w: &mut TcpStream, to: &str) {
    let _ = write!(
        w,
        "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    );
    let _ = w.flush();
}

fn serve(sh: &Arc<Shared>, conn: TcpStream) {
    let Ok(mut w) = conn.try_clone() else { return };
    let mut r = BufReader::new(conn);
    let Some(req) = ws::read_request(&mut r) else {
        return;
    };
    if req.method != "GET" {
        let _ = ws::respond(
            &mut w,
            "405 Method Not Allowed",
            "text/plain",
            b"GET only\n",
        );
        return;
    }
    let prefix = format!("/{}/", sh.token);
    let Some(rest) = req.route().strip_prefix(&prefix).map(str::to_owned) else {
        let _ = ws::respond(&mut w, "404 Not Found", "text/plain", b"not found\n");
        return;
    };
    match rest.as_str() {
        "" | "index.html" => {
            let _ = ws::respond(&mut w, "200 OK", "text/html; charset=utf-8", INDEX);
        }
        "dir.js" => {
            let _ = ws::respond(&mut w, "200 OK", "text/javascript; charset=utf-8", SCRIPT);
        }
        "ping" => {
            let _ = ws::respond(&mut w, "200 OK", "text/plain", b"phitex watch DIR\n");
        }
        "ws" if req.is_upgrade() => {
            // (`?doc=ID`: a document's viewer asking, its Files panel)
            let doc = req
                .path
                .split_once('?')
                .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("doc=")))
                .and_then(|id| sh.rel_of(id));
            if let Ok(sock) = ws::upgrade(r.into_inner(), &req) {
                index_socket(sh, sock, doc);
            }
        }
        d if d.starts_with("raw/") => raw(sh, &d[4..], &mut w),
        d if d.starts_with("d/") => {
            let (id, sub) = d[2..].split_once('/').unwrap_or((&d[2..], ""));
            let (id, sub) = (id.to_owned(), sub.to_owned());
            document(sh, &id, &sub, &req, r, w);
        }
        _ => {
            let _ = ws::respond(&mut w, "404 Not Found", "text/plain", b"not found\n");
        }
    }
}

/// A PDF or a picture of the folder, as it is (`raw/PATH`).
fn raw(sh: &Shared, path: &str, w: &mut TcpStream) {
    let not = |w: &mut TcpStream| {
        let _ = ws::respond(
            w,
            "404 Not Found",
            "text/plain",
            b"not a file of this folder\n",
        );
    };
    let Some(rel) = unescape(path) else {
        return not(w);
    };
    let Some(ctype) = media_type(&rel) else {
        return not(w);
    };
    let Ok(p) = inside(&sh.root, &rel) else {
        return not(w);
    };
    match std::fs::read(&p) {
        Ok(b) => {
            let _ = ws::respond(w, "200 OK", ctype, &b);
        }
        Err(_) => not(w),
    }
}

/// A page saying why a document could not be opened, with the way back.
fn failed_page(rel: &str, why: &str) -> Vec<u8> {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{} · phitex watch</title><style>body{{margin:0;min-height:100vh;display:grid;place-items:center;font:15px/1.5 system-ui,sans-serif;background:#f6f7f9;color:#1b1f27}}@media (prefers-color-scheme: dark){{body{{background:#14171c;color:#e4e7ee}}a{{color:#8ab4ff}}}}main{{max-width:34em;padding:24px}}h1{{font-size:18px;margin:0 0 8px}}p{{margin:0 0 16px;opacity:.8}}</style></head><body><main><h1>{} could not be opened</h1><p>{}</p><a href=\"../../\">Back to the folder</a></main></body></html>",
        esc(rel),
        esc(rel),
        esc(why)
    )
    .into_bytes()
}

/// A request under `d/ID/`: the viewer's page (the document opened: its
/// session started), else its session's answer.
fn document(
    sh: &Arc<Shared>,
    id: &str,
    sub: &str,
    req: &ws::Request,
    r: BufReader<TcpStream>,
    mut w: TcpStream,
) {
    let Some(rel) = sh
        .rel_of(id)
        .filter(|rel| kind_of(rel) == "tex" && inside(&sh.root, rel).is_ok_and(|p| p.is_file()))
    else {
        let _ = ws::respond(
            &mut w,
            "404 Not Found",
            "text/html; charset=utf-8",
            &failed_page(
                "This document",
                "It is not a .tex file of this folder (any more).",
            ),
        );
        return;
    };
    match sub {
        "" | "index.html" => {
            // (a part naming its root: the root opened, its page saying why)
            let (open, why) = root_of(&sh.root, &rel);
            if open != rel {
                let to = doc_id(&open);
                lock(&sh.st).ids.insert(to.clone(), open);
                let q = why
                    .map(|w| format!("?note={}", escape(&w)))
                    .unwrap_or_default();
                return redirect(&mut w, &format!("../{to}/{q}"));
            }
            if let Err(e) = connect(sh, &rel, true) {
                sh.ui.error(&e);
                let _ = ws::respond(
                    &mut w,
                    "500 Internal Server Error",
                    "text/html; charset=utf-8",
                    &failed_page(&rel, &e),
                );
                return;
            }
            let prefs = crate::config::prefs(None).ok();
            let page = crate::view::index_with(prefs.as_ref());
            let _ = ws::respond(&mut w, "200 OK", "text/html; charset=utf-8", &page);
        }
        "viewer.js" => {
            let _ = ws::respond(
                &mut w,
                "200 OK",
                "text/javascript; charset=utf-8",
                crate::view::SCRIPT,
            );
        }
        // (a tab of a session that ended on its own waits until the
        // document is opened again)
        "ping" => {
            let held = lock(&sh.st).docs.get(&rel).is_none_or(|d| d.held);
            let _ = if held {
                ws::respond(
                    &mut w,
                    "503 Service Unavailable",
                    "text/plain",
                    b"not running\n",
                )
            } else {
                let body = format!("phitex watch\nviewer {}\n", crate::view::viewer_hash());
                ws::respond(&mut w, "200 OK", "text/plain", body.as_bytes())
            };
        }
        "ws" if req.is_upgrade() => {
            if connect(sh, &rel, false).is_err() {
                let _ = ws::respond(
                    &mut w,
                    "503 Service Unavailable",
                    "text/plain",
                    b"not running\n",
                );
                return;
            }
            let Some(url) = session_url(sh, &rel) else {
                let _ = ws::respond(
                    &mut w,
                    "503 Service Unavailable",
                    "text/plain",
                    b"no session\n",
                );
                return;
            };
            // (a tab: counted while its WebSocket lasts; the last gone, the
            // session paused)
            let k = TABS.fetch_add(1, Ordering::Relaxed);
            if let Ok(c) = w.try_clone() {
                let mut st = lock(&sh.st);
                if let Some(d) = st.docs.get_mut(&rel) {
                    d.tabs.push((k, c));
                }
                sh.changed(&mut st);
            }
            let _ = proxy(r, req, &url, sub);
            let mut st = lock(&sh.st);
            if let Some(d) = st.docs.get_mut(&rel) {
                d.tabs.retain(|(j, _)| *j != k);
                if d.tabs.is_empty() {
                    disconnect(sh, &mut st, &rel);
                } else {
                    sh.changed(&mut st);
                }
            }
        }
        _ => match session_url(sh, &rel) {
            Some(url) => {
                let _ = proxy(r, req, &url, sub);
            }
            None => {
                let _ = ws::respond(
                    &mut w,
                    "503 Service Unavailable",
                    "text/plain",
                    b"not running\n",
                );
            }
        },
    }
}

/// The tabs' WebSockets, numbered.
static TABS: AtomicU64 = AtomicU64::new(0);

/// Document `rel`'s session's viewer address, waited for while its
/// session starts; none once it has no session.
fn session_url(sh: &Shared, rel: &str) -> Option<String> {
    let mut st = lock(&sh.st);
    loop {
        let run = st.docs.get(rel)?.run.as_ref()?;
        if let Some(u) = &run.url {
            return Some(u.clone());
        }
        st = sh.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
    }
}

/// The request passed to the session's server at `base`
/// (`http://127.0.0.1:PORT/TOKEN/`) as `sub`, and its bytes both ways
/// until either side closes (a response, or a WebSocket's life).
fn proxy(r: BufReader<TcpStream>, req: &ws::Request, base: &str, sub: &str) -> std::io::Result<()> {
    let bad = || std::io::Error::other("a session's address");
    let (host, path) = base
        .strip_prefix("http://")
        .and_then(|b| b.split_once('/'))
        .ok_or_else(bad)?;
    let mut up = TcpStream::connect(host)?;
    let _ = up.set_nodelay(true);
    let query = req
        .path
        .split_once('?')
        .map(|q| format!("?{}", q.1))
        .unwrap_or_default();
    let mut head = format!("{} /{path}{sub}{query} HTTP/1.1\r\n", req.method);
    for (k, v) in &req.headers {
        if k != "host" {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    head.push_str(&format!("host: {host}\r\n\r\n"));
    up.write_all(head.as_bytes())?;
    // (what the browser sent past the request's head, then the rest)
    up.write_all(r.buffer())?;
    let mut client = r.into_inner();
    let _ = client.set_read_timeout(None);
    let (mut up_r, mut client_w) = (up.try_clone()?, client.try_clone()?);
    let back = std::thread::spawn(move || {
        let _ = std::io::copy(&mut up_r, &mut client_w);
        let _ = client_w.shutdown(Shutdown::Both);
    });
    let _ = std::io::copy(&mut client, &mut up);
    let _ = up.shutdown(Shutdown::Both);
    let _ = back.join();
    Ok(())
}

/// Folder `dir`'s listing for the file manager: its entries, each
/// document's state where it was opened this run.
fn folder(sh: &Shared, dir: &str) -> Result<Value, String> {
    let entries = list(&sh.root, dir)?;
    let mut st = lock(&sh.st);
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let path = if dir.is_empty() {
            e.name.clone()
        } else {
            format!("{dir}/{}", e.name)
        };
        let kind = if e.dir { "dir" } else { kind_of(&e.name) };
        let mut v = json!({
            "name": e.name,
            "path": path,
            "kind": kind,
            "mtime": e.mtime,
            "size": e.size,
        });
        if kind == "tex" {
            let id = doc_id(&path);
            st.ids.insert(id.clone(), path.clone());
            v["id"] = json!(id);
            if let Some(d) = st.docs.get(&path) {
                v["doc"] = d.json();
            }
        }
        out.push(v);
    }
    Ok(json!({
        "event": "folder",
        "root": root_name(&sh.root.path),
        "dir": dir,
        "entries": out,
    }))
}

fn root_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The file manager's WebSocket (the index's, a viewer's Files panel):
/// the folder it shows sent again whenever it changes; its requests:
/// `list` a folder, `find` every document (quick open). `doc`: the
/// document of the viewer asking, which it starts in.
fn index_socket(sh: &Arc<Shared>, mut sock: WebSocket<TcpStream>, doc: Option<String>) {
    let mut dir: Option<String> = None;
    let hello = json!({
        "event": "hello",
        "root": root_name(&sh.root.path),
        "where": crate::cache::home_relative(&sh.root.path),
        "doc": doc.as_ref().map(|r| json!({"path": r, "id": doc_id(r), "dir": parent(r)})),
    });
    if sock.send(Message::text(hello.to_string())).is_err() {
        return;
    }
    // (what was sent last, and the versions it was made at)
    let mut sent = String::new();
    let mut at = (u64::MAX, u64::MAX);
    loop {
        match sock.read() {
            Ok(Message::Text(t)) => {
                let req: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
                let reply = match req["op"].as_str().unwrap_or("") {
                    "list" => {
                        let want = req["dir"].as_str().unwrap_or("");
                        match folder(sh, want) {
                            Ok(v) => {
                                dir = Some(want.to_owned());
                                sent = v.to_string();
                                at = (sh.files.load(Ordering::Acquire), lock(&sh.st).version);
                                Some(sent.clone())
                            }
                            Err(e) => Some(
                                json!({"event": "error", "dir": want, "message": e}).to_string(),
                            ),
                        }
                    }
                    "find" => {
                        let files: Vec<Value> = sh
                            .found()
                            .into_iter()
                            .map(|p| {
                                let id = doc_id(&p);
                                lock(&sh.st).ids.insert(id.clone(), p.clone());
                                json!({"path": p, "id": id})
                            })
                            .collect();
                        Some(json!({"event": "found", "files": files}).to_string())
                    }
                    _ => None,
                };
                if let Some(m) = reply
                    && sock.send(Message::text(m)).is_err()
                {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Err(e) if ws::timed_out(&e) => {}
            Err(_) => break,
            Ok(_) => {}
        }
        // (the folder shown changed, or a document's state: sent again
        // when what it shows differs)
        let Some(d) = dir.clone() else { continue };
        let now = (sh.files.load(Ordering::Acquire), lock(&sh.st).version);
        if now == at {
            continue;
        }
        at = now;
        let m = if let Ok(v) = folder(sh, &d) {
            v.to_string()
        } else {
            // (the folder gone: the nearest one still there)
            let mut up = parent(&d).to_owned();
            while list(&sh.root, &up).is_err() && !up.is_empty() {
                up = parent(&up).to_owned();
            }
            dir = Some(up.clone());
            at = (u64::MAX, u64::MAX);
            let Ok(v) = folder(sh, &up) else { continue };
            v.to_string()
        };
        if m != sent {
            if sock.send(Message::text(m.clone())).is_err() {
                break;
            }
            sent = m;
        }
    }
}

// ---------------------------------------------------------------------
// The watch's loop: the folder's changes, the keys, the status lines.

/// Whether a change of `p` may change a listing: not inside a hidden
/// folder, not the cache.
fn lists(root: &Root, p: &Path) -> bool {
    let Ok(rel) = p.strip_prefix(&root.path) else {
        return false;
    };
    // (a hidden file is not listed either: its folder's listing does not
    // change with it)
    if rel
        .components()
        .any(|c| hidden(&c.as_os_str().to_string_lossy()))
    {
        return false;
    }
    root.cache.as_ref().is_none_or(|k| !p.starts_with(k))
}

/// A line on standard input (a pipe): its key and the document it names
/// (`r`, `r paper/main.tex`, `quit`).
fn line_of(l: &str) -> (Key, Option<String>) {
    let l = l.trim();
    let (k, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
    let rest = rest.trim();
    let doc = (!rest.is_empty()).then(|| rest.to_owned());
    let key = match k {
        "" => Key::Rebuild,
        "quit" => Key::Quit,
        k if k.chars().count() == 1 => {
            let c = k.chars().next().unwrap_or(' ');
            Key::of(c).unwrap_or(Key::Other(c))
        }
        k => Key::Other(k.chars().next().unwrap_or(' ')),
    };
    (key, doc)
}

/// Standard input's keys (a terminal's) or lines (a pipe's), to `tx`. A
/// terminal's end quits; a pipe's does not (as `phitex watch FILE`).
fn read_keys(tx: &mpsc::Sender<Ev>, raw: bool) {
    if raw {
        let mut bytes = std::io::stdin().lock().bytes();
        while let Some(Ok(b)) = bytes.next() {
            if b == 0x1b {
                if let Some(Ok(b'[' | b'O')) = bytes.next() {
                    for b in bytes.by_ref() {
                        if b.as_ref()
                            .map_or(true, |b| b.is_ascii_alphabetic() || *b == b'~')
                        {
                            break;
                        }
                    }
                }
                continue;
            }
            let k = Key::of(char::from(b));
            if k == Some(Key::Suspend) {
                keys::suspend();
                continue;
            }
            let k = k.unwrap_or(Key::Other(char::from(b)));
            if tx.send(Ev::Key(k, None)).is_err() {
                return;
            }
        }
        let _ = tx.send(Ev::Key(Key::Quit, None));
    } else {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            let (k, doc) = line_of(&l);
            if tx.send(Ev::Key(k, doc)).is_err() {
                return;
            }
        }
    }
}

/// The documents with a session, in their status lines' order (the
/// numbers `1`-`9` choose).
fn running(st: &State) -> Vec<String> {
    st.docs
        .iter()
        .filter(|(_, d)| d.run.as_ref().is_some_and(|r| !r.stopping))
        .map(|(r, _)| r.clone())
        .collect()
}

/// The status lines: the watch's, then one an open document.
fn statuses(sh: &Shared, chosen: Option<&str>, shown_dir: &str) {
    let st = lock(&sh.st);
    let open = running(&st);
    let shown = st.docs.values().filter(|d| !d.tabs.is_empty()).count();
    let mut lines = vec![(
        format!(
            "{shown_dir} · {} open, {shown} shown{}",
            open.len(),
            match chosen {
                Some(c) => format!(" · keys to {c}"),
                None => String::new(),
            }
        ),
        None,
    )];
    for (k, r) in open.iter().enumerate() {
        let d = &st.docs[r];
        let what = if d.queued {
            "waiting for another document's build".to_owned()
        } else if d.busy.is_empty() {
            d.status.clone()
        } else {
            d.busy.clone()
        };
        let shown = match d.tabs.len() {
            0 if d.on => "opening".to_owned(),
            0 => "no tab, kept warm".to_owned(),
            1 => "1 tab".to_owned(),
            n => format!("{n} tabs"),
        };
        let mark = if chosen == Some(r.as_str()) {
            "▸"
        } else {
            " "
        };
        let what = if what.is_empty() {
            "starting".to_owned()
        } else {
            what
        };
        lines.push((format!("{mark}{} {r} · {what} · {shown}", k + 1), d.ok));
    }
    drop(st);
    sh.ui.statuses(&lines);
}

/// Watch folder `dir` (`o`: the command line's options, `args` its words
/// after the program); never returns.
#[allow(clippy::too_many_lines)]
pub fn main(dir: &Path, o: &Options, args: &[String], ui: &Ui) -> ! {
    let st = ui.st;
    let fail = |e: &str| -> ! {
        ui.error(e);
        std::process::exit(crate::EXIT_USAGE);
    };
    if !o.view {
        fail(
            "a folder's watch shows it in the viewer: --no-view needs a file (phitex watch --no-view paper.tex)",
        );
    }
    let root = dir
        .canonicalize()
        .unwrap_or_else(|e| fail(&format!("can't watch {}: {e}", dir.display())));
    let shown_dir = o.file.clone().unwrap_or_else(|| ".".to_owned());
    let (tx, rx) = mpsc::channel::<Ev>();
    let listener =
        crate::view::listen(o.port, &root).unwrap_or_else(|e| fail(&format!("the viewer: {e}")));
    let port = listener
        .local_addr()
        .map_or_else(|e| fail(&format!("the viewer: {e}")), |a| a.port());
    let token = crate::view::doc_token(&root);
    let url = format!("http://127.0.0.1:{port}/{token}/");
    // (the lock the sessions build under, one at a time)
    let turns = std::env::temp_dir().join(format!("phitex-turns-{}", std::process::id()));
    let sh = Arc::new(Shared {
        root: Root::new(root.clone()),
        url: url.clone(),
        token,
        st: Mutex::new(State::default()),
        cv: Condvar::new(),
        files: AtomicU64::new(0),
        args: session_args(args, &shown_dir),
        turns: turns.clone(),
        tx: Mutex::new(tx.clone()),
        ui: ui.clone(),
        budget: o.memory.unwrap_or_else(default_budget),
    });
    {
        let s = sh.clone();
        std::thread::Builder::new()
            .name("phitex-dir-view".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { continue };
                    let _ = conn.set_nodelay(true);
                    let s = s.clone();
                    let _ = std::thread::Builder::new()
                        .name("phitex-dir-conn".into())
                        .spawn(move || serve(&s, conn));
                }
            })
            .unwrap_or_else(|e| fail(&format!("the viewer: {e}")));
    }
    let _watcher = {
        let s = sh.clone();
        let mut w = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
            let Ok(ev) = ev else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            if ev.paths.iter().any(|p| lists(&s.root, p)) {
                s.files.fetch_add(1, Ordering::AcqRel);
            }
        })
        .unwrap_or_else(|e| fail(&format!("can't watch {}: {e}", root.display())));
        w.watch(&root, RecursiveMode::Recursive)
            .unwrap_or_else(|e| fail(&format!("can't watch {}: {e}", root.display())));
        w
    };
    if !ui.quiet {
        let threads = crate::session::threads(o.threads);
        ui.line(&format!(
            "{} {}  {}  {}",
            st.bold("PhiTeX"),
            st.dim(crate::version()),
            st.bold(&format!("{shown_dir}/")),
            st.dim(&format!(
                "pick a .tex file in the browser: it builds on every save · one build at a time, {threads} thread{}",
                if threads == 1 { "" } else { "s" }
            ))
        ));
    }
    ui.line(&format!(
        "{}  {}  {}",
        st.dim("folder"),
        st.cyan(&url),
        st.dim("(o opens it)")
    ));
    ui.event(&json!({"event": "viewer", "url": url}));
    if o.open && !crate::view::open_browser(&url) {
        ui.warning(&format!("could not start a browser: open {url}"));
    }
    let raw = !ui.json && keys::raw();
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("phitex-dir-stdin".into())
            .spawn(move || read_keys(&tx, raw))
            .unwrap_or_else(|e| fail(&format!("a thread: {e}")));
    }
    drop(tx);
    if raw {
        ui.keys("h: keys");
    }
    let mut chosen: Option<String> = None;
    let quit = |sh: &Shared| -> ! {
        ui.finish();
        keys::restore();
        // (every session asked to end, as `q`; waited for)
        let mut st = lock(&sh.st);
        st.quitting = true;
        for d in st.docs.values_mut() {
            d.cut();
            d.stop();
        }
        while st.docs.values().any(|d| d.run.is_some()) {
            st = sh.cv.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
        drop(st);
        let _ = std::fs::remove_file(&sh.turns);
        std::process::exit(0);
    };
    loop {
        statuses(&sh, chosen.as_deref(), &shown_dir);
        let Ok(ev) = rx.recv() else { quit(&sh) };
        let Ev::Key(k, doc) = ev else { continue };
        // (the documents a key acts on: the one named, else the one
        // chosen, else every open one)
        let targets = |st: &State| -> Vec<String> {
            match doc.as_ref().or(chosen.as_ref()) {
                Some(r) => st
                    .docs
                    .get(r)
                    .filter(|d| d.run.is_some())
                    .map(|_| r.clone())
                    .into_iter()
                    .collect(),
                None => running(st),
            }
        };
        match k {
            Key::Quit => quit(&sh),
            Key::Other(c @ '1'..='9') => {
                let k = c as usize - '1' as usize;
                match running(&lock(&sh.st)).get(k) {
                    Some(r) => {
                        ui.line(&st.dim(&format!("keys go to {r} now (a: to every document)")));
                        chosen = Some(r.clone());
                    }
                    None => ui.line(&format!("phitex watch: no document {c} open")),
                }
            }
            Key::Other('a' | 'A' | '0') => {
                chosen = None;
                ui.line(&st.dim("keys go to every open document now"));
            }
            Key::Rebuild | Key::Problems | Key::Warnings | Key::Summary => {
                let line = match k {
                    Key::Rebuild => "r",
                    Key::Problems => "p",
                    Key::Warnings => "w",
                    _ => "s",
                };
                let mut s = lock(&sh.st);
                let ts = targets(&s);
                if ts.is_empty() {
                    ui.line("phitex watch: no document open (pick one in the browser)");
                }
                for r in &ts {
                    if let Some(d) = s.docs.get_mut(r) {
                        d.send(line);
                    }
                }
            }
            Key::Open => {
                let target = chosen
                    .as_ref()
                    .map(|r| format!("{}d/{}/", sh.url, doc_id(r)));
                let u = target.unwrap_or_else(|| sh.url.clone());
                if !crate::view::open_browser(&u) {
                    ui.warning(&format!("could not start a browser: open {u}"));
                }
            }
            Key::Clear => ui.clear(),
            Key::Help => {
                for (k, what) in HELP {
                    ui.line(&format!("  {}  {what}", st.bold(&format!("{k:>9}"))));
                }
            }
            Key::Suspend => {}
            Key::Other(c) => ui.line(&format!(
                "phitex watch: `{c}` is no key here (h lists them)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("phitex-dirwatch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (f, t) in files {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        }
        d.canonicalize().unwrap()
    }

    #[test]
    fn a_session_runs_where_its_settings_are() {
        let d = tree(
            "where",
            &[
                ("a/main.tex", ""),
                ("b/.latexmkrc", ""),
                ("b/c/main.tex", ""),
            ],
        );
        assert_eq!(session_dir(&d, "a/main.tex"), d);
        assert_eq!(session_dir(&d, "b/c/main.tex"), d.join("b"));
    }

    #[test]
    fn a_sessions_command_line() {
        let v = |a: &[&str]| a.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        assert_eq!(
            session_args(
                &v(&[
                    "watch", "-j", "4", "--open", "--port", "2000", "--memory", "1GB", "."
                ]),
                "."
            ),
            v(&["watch", "-j", "4"])
        );
        assert_eq!(
            session_args(&v(&["watch", "-o", "out", "out", "--json"]), "out"),
            v(&["watch", "-o", "out"])
        );
    }

    #[test]
    fn pipe_lines() {
        assert_eq!(line_of("r"), (Key::Rebuild, None));
        assert_eq!(line_of(""), (Key::Rebuild, None));
        assert_eq!(
            line_of("r a/main.tex"),
            (Key::Rebuild, Some("a/main.tex".into()))
        );
        assert_eq!(line_of("quit"), (Key::Quit, None));
    }

    #[test]
    fn addresses_escaped_and_back() {
        for p in ["a/b c/ü.pdf", "x%y/z#1.png", "plain/main.tex"] {
            assert_eq!(unescape(&escape(p)).as_deref(), Some(p));
        }
        assert_eq!(unescape("%2e%2e/x").as_deref(), Some("../x"));
        assert_eq!(unescape("%zz"), None);
    }

    #[test]
    fn roots_named_by_parts() {
        let d = tree(
            "roots",
            &[
                ("p/main.tex", "\\documentclass{article}"),
                ("p/ch/one.tex", "% !TEX root = ../main.tex\n\\section{One}"),
                ("p/ch/two.tex", "%!TEX root=../main\n"),
                ("p/ch/out.tex", "% !TEX root = ../../../x.tex\n"),
                ("p/ch/self.tex", "% !TEX root = self.tex\n"),
            ],
        );
        let root = Root::new(d);
        assert_eq!(root_of(&root, "p/ch/one.tex").0, "p/main.tex");
        assert!(
            root_of(&root, "p/ch/one.tex")
                .1
                .unwrap()
                .contains("one.tex names it")
        );
        assert_eq!(root_of(&root, "p/ch/two.tex").0, "p/main.tex");
        let (o, why) = root_of(&root, "p/ch/out.tex");
        assert_eq!(o, "p/ch/out.tex");
        assert!(why.unwrap().contains("not a .tex file in this folder"));
        assert_eq!(
            root_of(&root, "p/ch/self.tex"),
            ("p/ch/self.tex".into(), None)
        );
        assert_eq!(root_of(&root, "p/main.tex"), ("p/main.tex".into(), None));
    }
}
