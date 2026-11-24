//! `phitex watch DIR` end to end: the folder's bounds (no `..`, no absolute
//! path, no link out, nothing hidden), its listing and the listing's live
//! updates, a document's session started only when it is opened, a part's
//! `% !TEX root` followed, the memory budget ending the least recently
//! viewed session, and two documents building one after the other.
//!
//! The builds read TeX's files from TeX Live where it is installed, else
//! from Shelf (`--shelf`).

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

type Ws = tungstenite::WebSocket<TcpStream>;

/// A fresh directory for a test, outside any git repository's own files.
fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("dirwatch")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a scratch directory");
    d.canonicalize().expect("canonical")
}

fn write(root: &Path, files: &[(&str, &str)]) {
    for (f, t) in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().expect("a folder")).expect("a folder");
        std::fs::write(p, t).expect("a file");
    }
}

fn doc(body: &str) -> String {
    format!("\\documentclass{{article}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n")
}

/// TeX's files: TeX Live's where its `tex` is on the path, else Shelf's.
fn files_args() -> Vec<&'static str> {
    let tex = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("kpsewhich").is_file()));
    if tex { vec![] } else { vec!["--shelf"] }
}

/// A `phitex watch DIR` and its events (`--json`).
struct Watch {
    child: Child,
    rx: mpsc::Receiver<Value>,
    url: String,
    host: String,
    seen: Vec<Value>,
}

impl Watch {
    fn start(root: &Path, home: &Path, extra: &[&str]) -> Watch {
        let mut cmd = Command::new("timeout");
        cmd.arg("600").arg(env!("CARGO_BIN_EXE_phitex"));
        cmd.args(["watch", "--json", "-j", "2"])
            .args(files_args())
            .args(extra)
            .arg(".")
            .current_dir(root)
            .env("HOME", home)
            .env(
                "PHITEX_CACHE_DIR",
                PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache"),
            )
            .env("SOURCE_DATE_EPOCH", "0")
            .env("FORCE_SOURCE_DATE", "1")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("TEXINPUTS")
            .env_remove("PHITEX_COMPAT")
            .env("LATEXMKRCSYS", home.join("no-system-latexmkrc"))
            .env("BROWSER", "true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn().expect("phitex watch runs");
        let (tx, rx) = mpsc::channel();
        let out = child.stdout.take().expect("its output");
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&l) {
                    let _ = tx.send(v);
                }
            }
        });
        let mut w = Watch {
            child,
            rx,
            url: String::new(),
            host: String::new(),
            seen: Vec::new(),
        };
        let v = w
            .next(|v| v["event"] == "viewer", 60)
            .expect("the folder's address");
        v["url"].as_str().expect("a url").clone_into(&mut w.url);
        w.host = w
            .url
            .trim_start_matches("http://")
            .split('/')
            .next()
            .expect("host")
            .to_owned();
        w
    }

    /// The next event `want` takes, waited for `secs` at most.
    fn next(&mut self, want: impl Fn(&Value) -> bool, secs: u64) -> Option<Value> {
        let end = Instant::now() + Duration::from_secs(secs);
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(left) {
                Ok(v) => {
                    self.seen.push(v.clone());
                    if want(&v) {
                        return Some(v);
                    }
                }
                Err(_) => break,
            }
        }
        None
    }

    /// The token's path (`/TOKEN/`).
    fn base(&self) -> String {
        self.url
            .trim_start_matches("http://")
            .trim_start_matches(&self.host)
            .to_owned()
    }

    /// `GET path` (raw, after the host: `/TOKEN/…`): its status line, its
    /// headers and its body.
    fn get_raw(&self, path: &str) -> (String, String, Vec<u8>) {
        let mut s = TcpStream::connect(&self.host).expect("connect");
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.host
        )
        .expect("a request");
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let at = out
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or(out.len());
        let head = String::from_utf8_lossy(&out[..at]).into_owned();
        let body = out.get(at + 4..).unwrap_or_default().to_vec();
        let (status, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
        (status.to_owned(), headers.to_owned(), body)
    }

    fn get(&self, rel: &str) -> (String, String, Vec<u8>) {
        self.get_raw(&format!("{}{rel}", self.base()))
    }

    /// A WebSocket to `rel` under the token (`ws`, `d/ID/ws`).
    fn socket(&self, rel: &str) -> Ws {
        let tcp = TcpStream::connect(&self.host).expect("connect");
        let (ws, _) = tungstenite::client(format!("ws://{}{}{rel}", self.host, self.base()), tcp)
            .expect("a WebSocket");
        ws
    }

    fn key(&mut self, line: &str) {
        if let Some(i) = self.child.stdin.as_mut() {
            let _ = writeln!(i, "{line}");
        }
    }

    /// An event `want` takes among those seen already, else the next.
    fn seen_or_next(&mut self, want: impl Fn(&Value) -> bool, secs: u64) -> Option<Value> {
        if let Some(v) = self.seen.iter().find(|v| want(v)) {
            return Some(v.clone());
        }
        self.next(want, secs)
    }

    fn quit(mut self) {
        self.key("quit");
        let _ = self.child.wait();
    }
}

/// A test that failed leaves no watch behind (its sessions end with it:
/// their standard input closes).
impl Drop for Watch {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // (SIGTERM to `timeout`, which gives it to the watch: it ends
            // as `q` does; a SIGKILL would end `timeout` alone)
            let _ = Command::new("kill")
                .arg(self.child.id().to_string())
                .status();
            let _ = self.child.wait();
        }
    }
}

/// The index's next message that `want` takes (60 s at most).
fn read_until(ws: &mut Ws, want: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(60);
    ws.get_ref()
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("a timeout");
    while Instant::now() < end {
        match ws.read() {
            Ok(m) => {
                let Ok(t) = m.to_text() else { continue };
                let v: Value = serde_json::from_str(t).expect("JSON");
                if want(&v) {
                    return v;
                }
            }
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => panic!("the index's socket: {e}"),
        }
    }
    panic!("no such message in 60 s");
}

#[allow(clippy::needless_pass_by_value)]
fn ask(ws: &mut Ws, op: Value) -> Value {
    ws.send(tungstenite::Message::text(op.to_string()))
        .expect("a request");
    let dir = op["dir"].clone();
    read_until(ws, |v| {
        v["event"] == "error"
            || (v["event"] == "folder" && v["dir"] == dir)
            || (op["op"] == "find" && v["event"] == "found")
    })
}

fn names(v: &Value) -> Vec<String> {
    v["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| e["name"].as_str().unwrap_or("").to_owned())
        .collect()
}

fn id_of(v: &Value, name: &str) -> String {
    v["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|e| e["name"] == name)
        .and_then(|e| e["id"].as_str())
        .unwrap_or_else(|| panic!("{name} listed with an id: {v}"))
        .to_owned()
}

fn home(base: &Path) -> PathBuf {
    let h = base.join("home");
    std::fs::create_dir_all(&h).expect("a home");
    h
}

/// The folder's bounds: no folder or file above it, by `..`, an absolute
/// path, an escape, a link out or a hidden name, through the listing, the
/// raw files and the documents' addresses alike.
#[test]
#[allow(clippy::too_many_lines)]
fn a_folder_watch_never_leaves_its_folder() {
    let base = scratch("bounds");
    let root = base.join("docs");
    let outside = base.join("outside");
    write(
        &root,
        &[
            ("a/main.tex", &doc("A.")),
            ("a/fig.png", "not really a png"),
            (".git/config", "[core]"),
            (".secret.pdf", "%PDF hidden"),
        ],
    );
    write(
        &outside,
        &[
            ("secret.tex", &doc("Secret.")),
            ("secret.pdf", "%PDF-1.4 secret"),
        ],
    );
    std::os::unix::fs::symlink(&outside, root.join("out")).expect("a link out");
    std::os::unix::fs::symlink(outside.join("secret.pdf"), root.join("a/out.pdf"))
        .expect("a file link out");
    std::os::unix::fs::symlink(root.join("a"), root.join("in")).expect("a link in");
    let w = Watch::start(&root, &home(&base), &[]);
    let mut ix = w.socket("ws");
    read_until(&mut ix, |v| v["event"] == "hello");
    let top = ask(&mut ix, json!({"op": "list", "dir": ""}));
    // (no hidden name, no link out; a link inside is listed)
    assert_eq!(names(&top), ["a", "in"], "{top}");
    let a = ask(&mut ix, json!({"op": "list", "dir": "a"}));
    assert_eq!(names(&a), ["fig.png", "main.tex"], "{a}");
    for bad in [
        "..",
        "../outside",
        "a/..",
        "a/../..",
        "/",
        "/etc",
        "//etc",
        "out",
        "out/x",
        ".git",
        "a/./b",
        "a//b",
        "./a",
        "a\\..\\..",
        "nope",
        "a/main.tex",
    ] {
        let v = ask(&mut ix, json!({"op": "list", "dir": bad}));
        assert_eq!(v["event"], "error", "{bad:?} listed: {v}");
    }
    // (raw files: a picture inside is served, nothing outside or hidden)
    let (s, h, _) = w.get("raw/a/fig.png");
    assert!(s.contains("200"), "{s}");
    assert!(h.contains("image/png"), "{h}");
    for bad in [
        "raw/../outside/secret.pdf",
        "raw/%2e%2e/outside/secret.pdf",
        "raw/a/%2e%2e/%2e%2e/outside/secret.pdf",
        "raw/%2Fetc%2Fpasswd",
        "raw//etc/passwd",
        "raw/out/secret.pdf",
        "raw/a/out.pdf",
        "raw/.secret.pdf",
        "raw/.git/config",
        "raw/a/main.tex",
    ] {
        let (s, _, body) = w.get(bad);
        assert!(!s.contains("200"), "{bad} served: {s}");
        assert!(
            !String::from_utf8_lossy(&body).contains("secret"),
            "{bad}: {s}"
        );
    }
    let (s, _, body) = w.get_raw(&format!("{}../../etc/passwd", w.base()));
    assert!(
        !s.contains("200") && !String::from_utf8_lossy(&body).contains("root:"),
        "{s}"
    );
    // (a document's address names a .tex file of the folder, or nothing:
    // one outside, under its own id, is not opened)
    let outside_id = {
        let rel = "../outside/secret.tex";
        let h = fnv(rel.as_bytes());
        format!("{:012x}", h & 0xffff_ffff_ffff)
    };
    for id in [outside_id.as_str(), "000000000000", "..", "%2e%2e"] {
        let (s, _, _) = w.get(&format!("d/{id}/"));
        assert!(s.contains("404"), "d/{id}/ opened: {s}");
    }
    // (nor by the path its address names, the id that path's)
    for p in [
        "../outside/secret.tex",
        "%2e%2e/outside/secret.tex",
        "out/secret.tex",
    ] {
        let (s, _, _) = w.get(&format!("d/{outside_id}/?path={p}"));
        assert!(s.contains("404"), "d/{outside_id}/?path={p} opened: {s}");
        let id = format!(
            "{:012x}",
            fnv(p.replace("%2e", ".").as_bytes()) & 0xffff_ffff_ffff
        );
        let (s, _, _) = w.get(&format!("d/{id}/?path={p}"));
        assert!(s.contains("404"), "d/{id}/?path={p} opened: {s}");
    }
    // (quick open lists the folder's documents once each, the link in
    // leading to one already listed)
    let found = ask(&mut ix, json!({"op": "find"}));
    let paths: Vec<&str> = found["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    assert_eq!(paths, ["a/main.tex"], "{found}");
    w.quit();
}

/// FNV-1a 64 and its separator, as `phitex_live::tex::Fnv::add` (a
/// document's id).
fn fnv(b: &[u8]) -> u64 {
    let h = b.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &c| {
        (h ^ u64::from(c)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    (h ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3)
}

/// The listing: every file and folder (no file read to decide), and the
/// folder shown sent again as files come, go and are renamed.
#[test]
fn a_folder_watch_lists_files_as_they_change() {
    let base = scratch("listing");
    let root = base.join("docs");
    write(
        &root,
        &[
            ("paper/main.tex", &doc("P.")),
            ("paper/notes.txt", "n"),
            ("README", "r"),
        ],
    );
    let w = Watch::start(&root, &home(&base), &[]);
    let mut ix = w.socket("ws");
    let top = ask(&mut ix, json!({"op": "list", "dir": ""}));
    assert_eq!(names(&top), ["README", "paper"], "{top}");
    let p = ask(&mut ix, json!({"op": "list", "dir": "paper"}));
    assert_eq!(names(&p), ["main.tex", "notes.txt"], "{p}");
    let kinds: Vec<&str> = p["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["tex", "other"]);
    // (a document never opened has no state)
    assert!(p["entries"][0].get("doc").is_none(), "{p}");
    std::fs::write(root.join("paper/ch1.tex"), "\\section{One}").expect("a file");
    let v = read_until(&mut ix, |v| {
        v["event"] == "folder" && names(v).contains(&"ch1.tex".to_owned())
    });
    assert_eq!(names(&v), ["ch1.tex", "main.tex", "notes.txt"]);
    std::fs::rename(root.join("paper/ch1.tex"), root.join("paper/intro.tex")).expect("a rename");
    let v = read_until(&mut ix, |v| {
        v["event"] == "folder" && names(v).contains(&"intro.tex".to_owned())
    });
    assert_eq!(names(&v), ["intro.tex", "main.tex", "notes.txt"]);
    std::fs::remove_file(root.join("paper/notes.txt")).expect("a removal");
    let v = read_until(&mut ix, |v| {
        v["event"] == "folder" && !names(v).contains(&"notes.txt".to_owned())
    });
    assert_eq!(names(&v), ["intro.tex", "main.tex"]);
    // (a folder comes)
    std::fs::create_dir_all(root.join("paper/sub")).expect("a folder");
    let v = read_until(&mut ix, |v| {
        v["event"] == "folder" && names(v).contains(&"sub".to_owned())
    });
    let sub = v["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|e| e["name"] == "sub")
        .expect("sub");
    assert_eq!(sub["kind"], "dir", "{v}");
    // (the folder shown removed: the nearest one still there)
    std::fs::remove_dir_all(root.join("paper")).expect("a removal");
    let v = read_until(&mut ix, |v| v["event"] == "folder" && v["dir"] == "");
    assert_eq!(names(&v), ["README"], "{v}");
    w.quit();
}

/// Nothing is built until a document is opened; opening one starts its
/// session (its build's events carry its path), and a part's `% !TEX
/// root` opens its root instead, the page told why.
#[test]
#[allow(clippy::many_single_char_names)]
fn a_folder_watch_starts_a_session_when_a_document_opens() {
    let base = scratch("open");
    let root = base.join("docs");
    write(
        &root,
        &[
            ("p/main.tex", &doc("\\input{ch/one}")),
            ("p/ch/one.tex", "% !TEX root = ../main.tex\nOne."),
            ("q/main.tex", &doc("Q.")),
            ("r/main.tex", &doc("R.")),
        ],
    );
    let mut w = Watch::start(&root, &home(&base), &[]);
    let mut ix = w.socket("ws");
    let q = ask(&mut ix, json!({"op": "list", "dir": "q"}));
    assert_eq!(id_of(&q, "main.tex").len(), 12);
    let ch = ask(&mut ix, json!({"op": "list", "dir": "p/ch"}));
    let one = id_of(&ch, "one.tex");
    // (nothing started while only listed)
    assert!(
        w.next(|v| v["event"] == "started" || v["event"] == "session", 3)
            .is_none(),
        "a session before any document was opened: {:?}",
        w.seen
    );
    // (the part: its root opened instead, the page told why)
    let (s, h, _) = w.get(&format!("d/{one}/"));
    assert!(s.contains("302"), "{s}");
    let main_id = format!("{:012x}", fnv(b"p/main.tex") & 0xffff_ffff_ffff);
    let loc = h
        .lines()
        .find_map(|l| l.strip_prefix("Location: "))
        .expect("a location")
        .to_owned();
    assert!(
        loc.starts_with(&format!("../{main_id}/?path=p/main.tex&note=")),
        "{loc}"
    );
    assert!(loc.contains("one.tex"), "{loc}");
    assert!(
        w.next(|v| v["event"] == "session", 2).is_none(),
        "the part's session started: {:?}",
        w.seen
    );
    // (the root opened: its session, its build)
    let (s, _, body) = w.get(&format!("d/{main_id}/"));
    assert!(s.contains("200"), "{s}");
    assert!(String::from_utf8_lossy(&body).contains("viewer.js"));
    let v = w
        .next(|v| v["event"] == "session" && v["doc"] == "p/main.tex", 30)
        .expect("its session");
    assert_eq!(v["connected"], true, "{v}");
    let b = w
        .next(|v| v["event"] == "build" && v["doc"] == "p/main.tex", 300)
        .expect("its build");
    assert_eq!(b["ok"], true, "{b}");
    // (its state in the listing; q still never opened)
    let p = ask(&mut ix, json!({"op": "list", "dir": "p"}));
    let m = p["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|e| e["name"] == "main.tex")
        .expect("main");
    assert_eq!(m["doc"]["build"], "ok", "{p}");
    assert_eq!(m["doc"]["pages"], 1, "{p}");
    let q = ask(&mut ix, json!({"op": "list", "dir": "q"}));
    assert!(q["entries"][0].get("doc").is_none(), "{q}");
    assert!(
        !w.seen.iter().any(|v| v["doc"] == "q/main.tex"),
        "q started: {:?}",
        w.seen
    );
    // (a tab left open from the last watch: its address names its path,
    // nothing walked to find it; r, never listed, opened so)
    let rid = format!("{:012x}", fnv(b"r/main.tex") & 0xffff_ffff_ffff);
    let (s, _, _) = w.get(&format!("d/{rid}/"));
    assert!(s.contains("404"), "r found by a walk: {s}");
    let (s, _, _) = w.get(&format!("d/{rid}/?path=r/main.tex"));
    assert!(s.contains("200"), "r by its path: {s}");
    w.seen_or_next(|v| v["event"] == "build" && v["doc"] == "r/main.tex", 300)
        .expect("r's build");
    // (the viewer's own files served under the document's address)
    let (s, _, js) = w.get(&format!("d/{main_id}/viewer.js"));
    assert!(s.contains("200") && js.len() > 100_000, "{s}");
    let (s, _, ping) = w.get(&format!("d/{main_id}/ping"));
    assert!(
        s.contains("200") && String::from_utf8_lossy(&ping).contains("viewer "),
        "{s}"
    );
    w.quit();
}

/// The memory budget, tiny: a document whose last tab closed is ended
/// once the budget is past; the one a tab still shows is not; opened
/// again, the ended one builds again.
#[test]
fn a_folder_watch_ends_the_least_recently_viewed_past_its_memory() {
    let base = scratch("memory");
    let root = base.join("docs");
    write(
        &root,
        &[("a/main.tex", &doc("A.")), ("b/main.tex", &doc("B."))],
    );
    let mut w = Watch::start(&root, &home(&base), &["--memory", "1"]);
    let id = |r: &str| format!("{:012x}", fnv(r.as_bytes()) & 0xffff_ffff_ffff);
    let (a, b) = (id("a/main.tex"), id("b/main.tex"));
    let mut ix = w.socket("ws");
    let _ = ask(&mut ix, json!({"op": "list", "dir": "a"}));
    let _ = ask(&mut ix, json!({"op": "list", "dir": "b"}));
    // (a opened in a tab, built; its tab closed)
    assert!(w.get(&format!("d/{a}/")).0.contains("200"));
    let tab_a = w.socket(&format!("d/{a}/ws"));
    w.next(|v| v["event"] == "build" && v["doc"] == "a/main.tex", 300)
        .expect("a's build");
    // (b opened and shown: a, no tab, the least recently viewed, ends)
    assert!(w.get(&format!("d/{b}/")).0.contains("200"));
    let tab_b = w.socket(&format!("d/{b}/ws"));
    drop(tab_a);
    let ended = w
        .next(|v| v["event"] == "session" && v["ended"] == "memory", 300)
        .expect("a session ended past the budget");
    assert_eq!(ended["doc"], "a/main.tex", "{ended}");
    w.next(
        |v| v["event"] == "session" && v["doc"] == "a/main.tex" && v["running"] == false,
        60,
    )
    .expect("a's session gone");
    // (b, shown, kept through its build)
    w.seen_or_next(|v| v["event"] == "build" && v["doc"] == "b/main.tex", 300)
        .expect("b's build");
    assert!(
        !w.seen
            .iter()
            .any(|v| v["event"] == "session" && v["doc"] == "b/main.tex" && v["ended"] == "memory"),
        "b ended while shown: {:?}",
        w.seen
    );
    let a_state = ask(&mut ix, json!({"op": "list", "dir": "a"}));
    assert_eq!(a_state["entries"][0]["doc"]["live"], false, "{a_state}");
    // (a opened again: a session of its own again, built)
    assert!(w.get(&format!("d/{a}/")).0.contains("200"));
    let b2 = w
        .next(|v| v["event"] == "build" && v["doc"] == "a/main.tex", 300)
        .expect("a built again");
    assert_eq!(b2["ok"], true, "{b2}");
    drop(tab_b);
    w.quit();
}

/// Two documents opened together build one after the other: a build
/// starts once the other's is in (the second said to wait), never both
/// at once.
#[test]
fn a_folder_watch_builds_one_document_at_a_time() {
    let base = scratch("turns");
    let root = base.join("docs");
    // (long enough that the second asks while the first builds)
    let words = "words ".repeat(40);
    let long = (1..=400).fold(String::new(), |mut s, k| {
        use std::fmt::Write as _;
        let _ = writeln!(s, "Paragraph {k}: {words}\\par");
        s
    });
    write(
        &root,
        &[("a/main.tex", &doc(&long)), ("b/main.tex", &doc(&long))],
    );
    let mut w = Watch::start(&root, &home(&base), &[]);
    let id = |r: &str| format!("{:012x}", fnv(r.as_bytes()) & 0xffff_ffff_ffff);
    let mut ix = w.socket("ws");
    let _ = ask(&mut ix, json!({"op": "list", "dir": "a"}));
    let _ = ask(&mut ix, json!({"op": "list", "dir": "b"}));
    assert!(w.get(&format!("d/{}/", id("a/main.tex"))).0.contains("200"));
    assert!(w.get(&format!("d/{}/", id("b/main.tex"))).0.contains("200"));
    for d in ["a/main.tex", "b/main.tex"] {
        // (whichever built first, the other's build read after it)
        let b = w
            .seen_or_next(|v| v["event"] == "build" && v["doc"] == d, 600)
            .unwrap_or_else(|| panic!("{d}'s build"));
        assert_eq!(b["ok"], true, "{b}");
    }
    // (the turns as the sessions said them: never two at once, and the
    // one that asked while the other built waited)
    let mut holder: Option<String> = None;
    let mut waited = false;
    for v in &w.seen {
        let d = v["doc"].as_str().unwrap_or("").to_owned();
        match (v["event"].as_str().unwrap_or(""), v["on"].as_bool()) {
            ("queued", Some(true)) => waited = true,
            ("turn", Some(true)) => {
                assert!(
                    holder.is_none(),
                    "{d} built while {holder:?} did: {:?}",
                    w.seen
                );
                holder = Some(d);
            }
            ("turn", Some(false)) => {
                assert_eq!(holder.as_deref(), Some(d.as_str()), "{:?}", w.seen);
                holder = None;
            }
            _ => {}
        }
    }
    assert!(waited, "the second document never waited: {:?}", w.seen);
    // (an edit of each, on a pipe naming it: built in turn too)
    w.key("r a/main.tex");
    let r = w.next(|v| v["event"] == "build", 300).expect("r's build");
    assert_eq!(r["doc"], "a/main.tex", "{r}");
    w.quit();
}

/// A big folder starts at once: only its own listing's folder is watched
/// (one watch, not one a folder below), each other folder while a page
/// shows it; a change in a folder no page shows sends nothing.
#[test]
fn a_folder_watch_of_many_folders_starts_at_once_and_watches_what_is_shown() {
    let base = scratch("many");
    let root = base.join("docs");
    for a in 0..40 {
        for b in 0..50 {
            for c in 0..10 {
                std::fs::create_dir_all(root.join(format!("n{a}/m{b}/k{c}"))).expect("a folder");
            }
        }
    }
    write(&root, &[("main.tex", &doc("M."))]);
    let t0 = Instant::now();
    let mut w = Watch::start(&root, &home(&base), &[]);
    let took = t0.elapsed();
    eprintln!("22,041 folders: the address after {took:?}");
    assert!(took < Duration::from_secs(5), "{took:?} to start");
    let watching: Vec<&Value> = w.seen.iter().filter(|v| v["event"] == "watching").collect();
    assert_eq!(watching.len(), 1, "{:?}", w.seen);
    assert_eq!(watching[0]["folders"], 1, "{:?}", w.seen);
    // (a folder opened: watched while it is shown, alone)
    let mut ix = w.socket("ws");
    let v = ask(&mut ix, json!({"op": "list", "dir": "n3/m7"}));
    assert_eq!(names(&v).len(), 10, "{v}");
    let on = w.next(|v| v["event"] == "watching", 10).expect("watched");
    assert_eq!(on["folders"], 2, "{on}");
    // (a change below it, in a folder no page shows: nothing sent, though
    // that folder's time in this listing moved)
    std::fs::write(root.join("n3/m7/k4/deep.tex"), "x").expect("a file");
    ix.get_ref()
        .set_read_timeout(Some(Duration::from_millis(2000)))
        .expect("a timeout");
    match ix.read() {
        Err(tungstenite::Error::Io(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) => {}
        other => panic!("sent for a folder no page shows: {other:?}"),
    }
    // (a change in it: sent)
    std::fs::write(root.join("n3/m7/here.tex"), "x").expect("a file");
    let v = read_until(&mut ix, |v| {
        v["event"] == "folder" && names(v).contains(&"here.tex".to_owned())
    });
    assert_eq!(v["dir"], "n3/m7");
    // (the page gone: watched no more, the root still)
    drop(ix);
    let off = w.next(|v| v["event"] == "watching", 10).expect("unwatched");
    assert_eq!(off["folders"], 1, "{off}");
    w.quit();
}

/// The Running strip's rows, as the folder's listing carries them: each
/// running document's path, build and tabs.
fn running(v: &Value) -> Vec<(String, String, u64)> {
    v["running"]
        .as_array()
        .expect("running")
        .iter()
        .map(|d| {
            (
                d["path"].as_str().unwrap_or("").to_owned(),
                d["build"].as_str().unwrap_or("").to_owned(),
                d["tabs"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

fn id(rel: &str) -> String {
    format!("{:012x}", fnv(rel.as_bytes()) & 0xffff_ffff_ffff)
}

/// The Running strip lists the documents running with their state and
/// tabs; Stop ends one (its tabs cut), Stop all every one, `x` on a pipe
/// the one it names; a stopped document opened again starts and builds.
#[test]
#[allow(clippy::too_many_lines)]
fn a_folder_watch_lists_whats_running_and_stops_it() {
    let base = scratch("stop");
    let root = base.join("docs");
    write(
        &root,
        &[("a/main.tex", &doc("A.")), ("b/main.tex", &doc("B."))],
    );
    let mut w = Watch::start(&root, &home(&base), &[]);
    let mut ix = w.socket("ws");
    let top = ask(&mut ix, json!({"op": "list", "dir": ""}));
    assert!(running(&top).is_empty(), "{top}");
    assert!(
        w.get(&format!("d/{}/?path=a/main.tex", id("a/main.tex")))
            .0
            .contains("200")
    );
    let mut tab_a = w.socket(&format!("d/{}/ws", id("a/main.tex")));
    assert!(
        w.get(&format!("d/{}/?path=b/main.tex", id("b/main.tex")))
            .0
            .contains("200")
    );
    for d in ["a/main.tex", "b/main.tex"] {
        let b = w
            .seen_or_next(|v| v["event"] == "build" && v["doc"] == d, 300)
            .unwrap_or_else(|| panic!("{d}'s build"));
        assert_eq!(b["ok"], true, "{b}");
    }
    // (both in the strip, built, a in one tab)
    let v = read_until(&mut ix, |v| {
        v["event"] == "folder" && running(v).iter().filter(|r| r.1 == "ok").count() == 2
    });
    assert_eq!(
        running(&v),
        [
            ("a/main.tex".to_owned(), "ok".to_owned(), 1),
            ("b/main.tex".to_owned(), "ok".to_owned(), 0)
        ],
        "{v}"
    );
    assert_eq!(v["running"][0]["pages"], 1, "{v}");
    // (Stop on b's row: gone from the strip, its session ended)
    ix.send(tungstenite::Message::text(
        json!({"op": "stop", "path": "b/main.tex"}).to_string(),
    ))
    .expect("stop");
    let v = read_until(&mut ix, |v| v["event"] == "folder" && running(v).len() == 1);
    assert_eq!(running(&v)[0].0, "a/main.tex", "{v}");
    w.seen_or_next(
        |v| v["event"] == "session" && v["doc"] == "b/main.tex" && v["ended"] == "stopped",
        30,
    )
    .expect("b stopped");
    w.seen_or_next(
        |v| v["event"] == "session" && v["doc"] == "b/main.tex" && v["running"] == false,
        60,
    )
    .expect("b's session gone");
    // (b opened again: started, built)
    let builds = w
        .seen
        .iter()
        .filter(|v| v["event"] == "build" && v["doc"] == "b/main.tex")
        .count();
    assert!(
        w.get(&format!("d/{}/?path=b/main.tex", id("b/main.tex")))
            .0
            .contains("200")
    );
    w.next(|v| v["event"] == "build" && v["doc"] == "b/main.tex", 300)
        .expect("b built again");
    assert_eq!(
        w.seen
            .iter()
            .filter(|v| v["event"] == "build" && v["doc"] == "b/main.tex")
            .count(),
        builds + 1
    );
    let v = read_until(&mut ix, |v| v["event"] == "folder" && running(v).len() == 2);
    assert_eq!(running(&v)[1].1, "ok", "{v}");
    // (Stop all: nothing running, a's tab cut)
    ix.send(tungstenite::Message::text(
        json!({"op": "stop-all"}).to_string(),
    ))
    .expect("stop all");
    let v = read_until(&mut ix, |v| v["event"] == "folder" && running(v).is_empty());
    assert!(running(&v).is_empty(), "{v}");
    tab_a
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("a timeout");
    while tab_a.read().is_ok() {}
    for d in ["a/main.tex", "b/main.tex"] {
        w.seen_or_next(
            |v| v["event"] == "session" && v["doc"] == d && v["ended"] == "stopped",
            30,
        )
        .unwrap_or_else(|| panic!("{d} stopped"));
    }
    // (a tab of a stopped document is not let in until it is opened again)
    let (s, _, _) = w.get(&format!("d/{}/ping", id("a/main.tex")));
    assert!(s.contains("503"), "{s}");
    // (x on a pipe, naming a: a opened again, then stopped)
    assert!(
        w.get(&format!("d/{}/?path=a/main.tex", id("a/main.tex")))
            .0
            .contains("200")
    );
    w.next(|v| v["event"] == "build" && v["doc"] == "a/main.tex", 300)
        .expect("a built again");
    let stops = w
        .seen
        .iter()
        .filter(|v| v["event"] == "session" && v["doc"] == "a/main.tex" && v["ended"] == "stopped")
        .count();
    w.key("x a/main.tex");
    w.next(
        |v| v["event"] == "session" && v["doc"] == "a/main.tex" && v["ended"] == "stopped",
        30,
    )
    .expect("x stopped a");
    assert_eq!(
        w.seen
            .iter()
            .filter(|v| v["event"] == "session"
                && v["doc"] == "a/main.tex"
                && v["ended"] == "stopped")
            .count(),
        stops + 1
    );
    w.quit();
}

/// A document no tab shows stops by itself after `--auto-stop`; a tab
/// opened again before then cancels it; `--auto-stop -1` never stops it.
#[test]
#[allow(clippy::too_many_lines)]
fn a_folder_watch_stops_a_document_nobody_looks_at() {
    let base = scratch("autostop");
    let root = base.join("docs");
    write(
        &root,
        &[("a/main.tex", &doc("A.")), ("b/main.tex", &doc("B."))],
    );
    let mut w = Watch::start(&root, &home(&base), &["--auto-stop", "2s"]);
    let mut ix = w.socket("ws");
    let _ = ask(&mut ix, json!({"op": "list", "dir": ""}));
    // (a shown, then its tab closed: stops after 2 s)
    assert!(
        w.get(&format!("d/{}/?path=a/main.tex", id("a/main.tex")))
            .0
            .contains("200")
    );
    let tab = w.socket(&format!("d/{}/ws", id("a/main.tex")));
    w.seen_or_next(|v| v["event"] == "build" && v["doc"] == "a/main.tex", 300)
        .expect("a's build");
    drop(tab);
    let t0 = Instant::now();
    let v = w
        .next(
            |v| v["event"] == "session" && v["doc"] == "a/main.tex" && v["auto_stop"].is_number(),
            30,
        )
        .expect("a's auto-stop set");
    assert_eq!(v["auto_stop"], 2.0, "{v}");
    let strip = read_until(&mut ix, |v| {
        v["event"] == "folder"
            && v["running"]
                .as_array()
                .is_some_and(|r| r.iter().any(|d| d["stops_at"].is_number()))
    });
    assert_eq!(strip["running"][0]["tabs"], 0, "{strip}");
    w.next(
        |v| v["event"] == "session" && v["doc"] == "a/main.tex" && v["ended"] == "idle",
        30,
    )
    .expect("a stopped by itself");
    let took = t0.elapsed();
    assert!(
        took >= Duration::from_millis(1800) && took < Duration::from_secs(20),
        "{took:?}"
    );
    // (b shown, its tab closed, and a tab back within the wait: kept)
    assert!(
        w.get(&format!("d/{}/?path=b/main.tex", id("b/main.tex")))
            .0
            .contains("200")
    );
    let tab = w.socket(&format!("d/{}/ws", id("b/main.tex")));
    w.seen_or_next(|v| v["event"] == "build" && v["doc"] == "b/main.tex", 300)
        .expect("b's build");
    drop(tab);
    w.next(
        |v| v["event"] == "session" && v["doc"] == "b/main.tex" && v["auto_stop"].is_number(),
        30,
    )
    .expect("b's auto-stop set");
    let tab = w.socket(&format!("d/{}/ws", id("b/main.tex")));
    w.next(
        |v| {
            v["event"] == "session"
                && v["doc"] == "b/main.tex"
                && v["auto_stop"].is_null()
                && v.get("auto_stop").is_some()
        },
        30,
    )
    .expect("b's auto-stop cancelled");
    let stopped = w.next(
        |v| v["event"] == "session" && v["doc"] == "b/main.tex" && v.get("ended").is_some(),
        5,
    );
    assert!(
        stopped.is_none(),
        "b stopped though shown again: {stopped:?}"
    );
    drop(tab);
    w.quit();
    // (--auto-stop -1: a no tab shows keeps running)
    let mut w = Watch::start(&root, &home(&base), &["--auto-stop", "-1"]);
    let mut ix = w.socket("ws");
    let _ = ask(&mut ix, json!({"op": "list", "dir": ""}));
    assert!(
        w.get(&format!("d/{}/?path=a/main.tex", id("a/main.tex")))
            .0
            .contains("200")
    );
    let tab = w.socket(&format!("d/{}/ws", id("a/main.tex")));
    w.seen_or_next(|v| v["event"] == "build" && v["doc"] == "a/main.tex", 300)
        .expect("a's build");
    drop(tab);
    w.next(
        |v| v["event"] == "session" && v["doc"] == "a/main.tex" && v["connected"] == false,
        30,
    )
    .expect("a's tab gone");
    let gone = w.next(
        |v| v.get("auto_stop").is_some() || v.get("ended").is_some(),
        5,
    );
    assert!(gone.is_none(), "--auto-stop -1 stopped it: {gone:?}");
    // (the strip as it is now: the messages before it read first)
    let v = read_until(&mut ix, |v| {
        v["event"] == "folder" && running(v) == [("a/main.tex".to_owned(), "ok".to_owned(), 0)]
    });
    assert!(v["running"][0]["stops_at"].is_null(), "{v}");
    w.quit();
}
