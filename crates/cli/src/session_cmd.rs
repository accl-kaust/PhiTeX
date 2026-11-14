//! `phitex session`: one command for a running watch, for its user's
//! agent (docs/spec-editor-protocol.md §9.1). Each verb finds the watch
//! from the current directory, connects to its attach socket (§8) with
//! the token from its discovery file, makes one request (`phitex/*`,
//! [`crate::review::Hub::request`]) and prints the answer: short, one
//! line per problem, `--json` the result as it came. `notes` reads
//! `.phitex/notes.json` itself when no watch runs.
//!
//! Exit statuses: 0 done, 1 the build has TeX errors, 2 timed out, 3 no
//! running watch, 4 refused.

use std::io::{BufReader, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::review::{EXIT_NO_SESSION, EXIT_REFUSED, EXIT_TIMEOUT};
use crate::ui::{Style, Ui};

/// `phitex session`'s verbs.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    /// Start a watch in the background (or find the one running), once
    /// its first build is in: its pid, viewer and log
    ///
    /// Runs `phitex watch --write FILE` detached, its output in
    /// .phitex/session.log; WATCH-ARGS after `--` go to it.
    Start {
        /// The main .tex file (default: phitex.toml's `main`, or the one
        /// with a \documentclass here)
        #[arg(value_name = "FILE")]
        file: Option<String>,
        /// How long to wait for the first build (default 10m)
        #[arg(long, value_name = "TIME")]
        timeout: Option<String>,
        /// The watch's own options
        #[arg(last = true, value_name = "WATCH-ARGS")]
        args: Vec<String>,
    },
    /// Wait until a build has read the files as they are now
    ///
    /// Answers at once when the last build did; else waits for the build
    /// that does, never an older one.
    Wait {
        /// …and for its warnings (made when the editing pauses)
        #[arg(long)]
        warnings: bool,
        /// …and for the exact PDF written after it (a watch that writes
        /// files, --write, writes it when idle; `phitex session pdf` has
        /// any watch write it now)
        #[arg(long)]
        pdf: bool,
        /// Wait for the user instead: a note, a reply or a review in the
        /// viewer
        #[arg(long)]
        notes: bool,
        /// Give up after TIME (60s, 10m, 0: never; default 60s)
        #[arg(long, value_name = "TIME")]
        timeout: Option<String>,
    },
    /// The last build: its number, errors, pages, time
    Status,
    /// The last build's problems, one a line (file:line: message)
    Problems {
        #[arg(long)]
        errors: bool,
        #[arg(long)]
        warnings: bool,
        /// Only this file's
        #[arg(long, value_name = "FILE")]
        file: Option<String>,
        /// N source lines around each
        #[arg(long, value_name = "N")]
        context: Option<usize>,
        /// Counts instead: by kind (error, overfull, underfull, reference,
        /// font, other), each kind's files by count
        #[arg(long)]
        summary: bool,
    },
    /// Build again from the files on disk (--clean: from nothing)
    Rebuild {
        #[arg(long)]
        clean: bool,
    },
    /// The last build's log (its transcript)
    Log {
        /// Only its last N lines
        #[arg(long, value_name = "N")]
        tail: Option<usize>,
        #[arg(long, value_name = "N")]
        build: Option<u64>,
    },
    /// Write the exact PDF now, and print its path
    ///
    /// The last build's PDF, byte for byte the one a clean build makes
    /// (`phitex build`'s), also from a watch started without --write:
    /// JOB.pdf in the watch's output folder, as `phitex build` names it,
    /// or PATH; nothing else is written. Waits until written. A watch that
    /// writes files (--write) writes it when idle: that is waited for
    /// (`wait --pdf`), and copied to PATH.
    Pdf {
        /// Write it to PATH instead
        #[arg(short = 'o', long = "output", value_name = "PATH")]
        output: Option<String>,
        /// Its .synctex.gz too, beside it
        #[arg(long)]
        synctex: bool,
    },
    /// End the watch as q does (the exact PDF written first)
    Stop,
    /// The running watches
    List,
    /// The notes left in the viewer (open ones; --all: every one)
    Notes {
        #[arg(long)]
        open: bool,
        #[arg(long)]
        all: bool,
    },
    /// Answer a note
    Reply {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(value_name = "TEXT")]
        text: String,
    },
    /// Resolve a note: the source you changed since reading it is
    /// recorded in its thread
    Resolve {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(value_name = "TEXT")]
        text: Option<String>,
    },
    /// Your edits the user has not accepted yet, the verdicts and the
    /// comments on them
    Changes,
}

/// Why a verb could not be done, and the status it exits with.
struct Failed(i32, String);

fn no_session(m: impl Into<String>) -> Failed {
    Failed(EXIT_NO_SESSION, m.into())
}

/// `60s`, `10m`, `1h`, `500ms`, `0` (none), a bare number of seconds.
pub fn duration(s: &str) -> Result<Option<Duration>, String> {
    let s = s.trim();
    let (n, unit) = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .map_or((s, ""), |i| s.split_at(i));
    let v: f64 = n
        .parse()
        .map_err(|_| format!("{s:?}: a time such as 60s, 10m or 0"))?;
    let secs = match unit {
        "" | "s" => v,
        "ms" => v / 1000.0,
        "m" => v * 60.0,
        "h" => v * 3600.0,
        _ => return Err(format!("{s:?}: a time such as 60s, 10m or 0")),
    };
    Ok((secs > 0.0).then(|| Duration::from_secs_f64(secs)))
}

/// The watch for here: `pid` if given, else those whose root holds the
/// current directory, the one whose main is `file` first, else the newest.
/// The watch `which` names (`--session`, PHITEX_SESSION): a pid, or a
/// viewer's URL, whose `session.json` is the watch's discovery object;
/// none: the one running here ([`find_here`]).
fn find(which: Option<&str>, file: Option<&str>) -> Result<Value, Failed> {
    match which {
        None => find_here(None, file),
        Some(w) if w.starts_with("http://") => by_url(w),
        Some(w) => match w.parse::<u32>() {
            Ok(pid) => find_here(Some(pid), file),
            Err(_) => Err(Failed(
                EXIT_REFUSED,
                format!(
                    "--session {w}: neither a pid nor a viewer's URL (http://127.0.0.1:PORT/TOKEN/)"
                ),
            )),
        },
    }
}

/// The watch whose viewer is at `url` (`http://127.0.0.1:PORT/TOKEN/…`):
/// its discovery object from `/TOKEN/session.json`.
fn by_url(url: &str) -> Result<Value, Failed> {
    let rest = url.trim_start_matches("http://");
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let token = path.split('/').next().unwrap_or("");
    let bad = || {
        no_session(format!(
            "{url}: not a phitex viewer's URL (http://127.0.0.1:PORT/TOKEN/)"
        ))
    };
    if token.is_empty() || !host.contains(':') {
        return Err(bad());
    }
    let mut s =
        TcpStream::connect(host).map_err(|e| no_session(format!("the viewer at {host}: {e}")))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    write!(
        s,
        "GET /{token}/session.json HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| no_session(format!("the viewer at {host}: {e}")))?;
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut s, &mut body)
        .map_err(|e| no_session(format!("the viewer at {host}: {e}")))?;
    let text = String::from_utf8_lossy(&body);
    let (head, json) = text.split_once("\r\n\r\n").ok_or_else(bad)?;
    if !head.starts_with("HTTP/1.1 200") {
        return Err(no_session(format!(
            "{url}: the viewer has no session for that (a watch of an older phitex, or a wrong token): {}",
            head.lines().next().unwrap_or("")
        )));
    }
    let mut v: Value = serde_json::from_str(json.trim()).map_err(|_| bad())?;
    // (the socket at the viewer's host: the same machine as seen from here)
    if let Some(h) = host.rsplit_once(':').map(|(h, _)| h) {
        v["lsp"]["host"] = json!(h);
    }
    Ok(v)
}

fn find_here(pid: Option<u32>, file: Option<&str>) -> Result<Value, Failed> {
    // (every folder a watch may have said itself in: a sandbox may see
    // the user's runtime folder without $XDG_RUNTIME_DIR, or not at all)
    let folders = crate::attach::read_folders();
    let live: Vec<Value> = folders
        .iter()
        .flat_map(|(d, _)| crate::attach::live_in(d))
        .collect();
    if let Some(p) = pid {
        return live
            .into_iter()
            .find(|v| v["pid"].as_u64() == Some(u64::from(p)))
            .ok_or_else(|| no_session(format!("no phitex watch with pid {p} runs")));
    }
    let here = std::env::current_dir()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_default();
    let want = file.map(|f| {
        let p = here.join(f);
        p.canonicalize().unwrap_or(p)
    });
    let mut mine: Vec<Value> = live
        .into_iter()
        .filter(|v| {
            v["root"]
                .as_str()
                .is_some_and(|r| here.starts_with(Path::new(r)))
        })
        .collect();
    if let Some(w) = &want
        && let Some(v) = mine.iter().rev().find(|v| {
            v["main"]
                .as_str()
                .is_some_and(|m| Path::new(m) == w.as_path())
        })
    {
        return Ok(v.clone());
    }
    if want.is_some() {
        return Err(no_session(format!(
            "no phitex watch of {} runs here: phitex session start {}",
            file.unwrap_or(""),
            file.unwrap_or("")
        )));
    }
    mine.pop()
        .ok_or_else(|| no_session(not_found(&here, &folders)))
}

/// Why no watch was found from `here`: where the discovery files were
/// looked for, and a watch of this folder that runs but cannot be reached
/// from here (another PID namespace, its socket not answering: a sandbox
/// without the host's network).
fn not_found(here: &Path, folders: &[(PathBuf, String)]) -> String {
    let unreachable = folders
        .iter()
        .flat_map(|(d, _)| crate::attach::files_in(d))
        .map(|(_, v)| v)
        .find(|v| {
            !crate::attach::same_net(v)
                && v["root"]
                    .as_str()
                    .is_some_and(|r| here.starts_with(Path::new(r)))
        });
    if let Some(v) = unreachable {
        return format!(
            "a phitex watch of this folder runs (pid {} in another sandbox or outside this one) but its socket, 127.0.0.1:{}, does not answer from here: \
             the sandbox has no network to the host's 127.0.0.1 (bubblewrap: --share-net)",
            v["pid"], v["lsp"]["port"]
        );
    }
    let looked: Vec<String> = folders
        .iter()
        .map(|(d, why)| format!("{} ({why})", d.display()))
        .collect();
    format!(
        "no phitex watch runs here (looked in {}): start one with `phitex session start`, \
         or, if one runs in your terminal and this is a sandbox: --session <its viewer's URL> (or PHITEX_SESSION=<URL> once), \
         or make its sessions folder visible here (read-only will do)",
        looked.join(", ")
    )
}

/// A connection to a watch's socket, `initialize`d.
struct Conn {
    r: BufReader<TcpStream>,
    w: TcpStream,
    next: u64,
}

fn send(w: &mut TcpStream, v: &Value) -> std::io::Result<()> {
    let body = v.to_string();
    write!(w, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    w.flush()
}

impl Conn {
    fn open(v: &Value) -> Result<Conn, Failed> {
        let port = v["lsp"]["port"].as_u64().unwrap_or(0);
        let token = v["lsp"]["token"].as_str().unwrap_or("");
        let s = TcpStream::connect(("127.0.0.1", u16::try_from(port).unwrap_or(0)))
            .map_err(|e| no_session(format!("the watch's socket: {e}")))?;
        let _ = s.set_nodelay(true);
        let w = s.try_clone().map_err(|e| no_session(e.to_string()))?;
        let mut c = Conn {
            r: BufReader::new(s),
            w,
            next: 1,
        };
        let root = v["root"].as_str().unwrap_or("");
        c.call(
            "initialize",
            &json!({"processId": std::process::id(), "rootUri": phitex_live::lsp::uri_of(Path::new(root)),
                    "capabilities": {}, "initializationOptions": {"token": token, "session": true}}),
            None,
        )?;
        Ok(c)
    }

    /// Request `method`: its result (the notifications in between passed
    /// over); `within`: the time to wait for it.
    fn call(
        &mut self,
        method: &str,
        params: &Value,
        within: Option<Duration>,
    ) -> Result<Value, Failed> {
        let id = self.next;
        self.next += 1;
        send(
            &mut self.w,
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .map_err(|e| no_session(format!("the watch's socket: {e}")))?;
        let _ = self.r.get_ref().set_read_timeout(within);
        loop {
            let Some(m) = crate::lsp::read(&mut self.r) else {
                return Err(no_session("the watch ended (its socket closed)"));
            };
            if m["id"].as_u64() != Some(id) || m.get("method").is_some() {
                continue;
            }
            if let Some(e) = m.get("error") {
                let status = e["data"]["status"]
                    .as_i64()
                    .and_then(|s| i32::try_from(s).ok())
                    .unwrap_or(EXIT_REFUSED);
                return Err(Failed(
                    status,
                    e["message"].as_str().unwrap_or("refused").to_owned(),
                ));
            }
            return Ok(m["result"].clone());
        }
    }
}

/// `phitex session VERB`: its exit status.
pub fn main(verb: &Verb, which: Option<&str>, as_json: bool, ui: &Ui) -> i32 {
    let st = Style::stdout();
    let env = std::env::var("PHITEX_SESSION")
        .ok()
        .filter(|v| !v.is_empty());
    let which = which.or(env.as_deref());
    match run(verb, which, as_json, st, ui) {
        Ok(code) => code,
        Err(Failed(code, m)) => {
            if as_json {
                println!("{}", json!({"error": m, "status": code}));
            } else {
                ui.error(&m);
            }
            code
        }
    }
}

fn run(verb: &Verb, pid: Option<&str>, as_json: bool, st: Style, ui: &Ui) -> Result<i32, Failed> {
    let out = |v: &Value| println!("{v}");
    match verb {
        Verb::List => Ok(crate::attach::sessions(ui, as_json)),
        Verb::Start {
            file,
            timeout,
            args,
        } => start(file.as_deref(), timeout.as_deref(), args, as_json, st),
        Verb::Notes { open: _, all } => {
            // (no watch: the notes file read as it is)
            let v = if let Ok(mut c) = find(pid, None).and_then(|s| Conn::open(&s)) {
                c.call("phitex/notes/list", &json!({"all": all}), None)?
            } else {
                let root = std::env::current_dir().unwrap_or_default();
                let s = crate::review::Store::load(&project_root(&root))
                    .map_err(|e| Failed(EXIT_REFUSED, e))?;
                let notes: Vec<Value> = s
                    .notes
                    .iter()
                    .filter(|n| !n.draft && (*all || n.status == "open"))
                    .map(|n| json!(n))
                    .collect();
                json!({"notes": notes, "offline": true})
            };
            if as_json {
                out(&v);
            } else {
                print_notes(&v["notes"], st);
            }
            Ok(0)
        }
        _ => {
            let s = find(pid, None)?;
            let mut c = Conn::open(&s)?;
            match verb {
                Verb::Status => {
                    let v = c.call("phitex/session/status", &json!({}), None)?;
                    if as_json {
                        out(&v);
                    } else {
                        if let Some(l) = setup_line(&v) {
                            println!("{l}");
                        }
                        print_status(&v, st, None);
                    }
                    Ok(build_code(&v))
                }
                Verb::Wait {
                    warnings,
                    pdf,
                    notes,
                    timeout,
                } => {
                    let t = match timeout.as_deref() {
                        Some(t) => duration(t).map_err(|e| Failed(EXIT_REFUSED, e))?,
                        None => Some(Duration::from_secs(60)),
                    };
                    let ms = t.map_or(0, |d| d.as_millis());
                    let v = c.call(
                        "phitex/session/wait",
                        &json!({"warnings": warnings, "pdf": pdf, "notes": notes, "timeout_ms": ms}),
                        t.map(|d| d + Duration::from_secs(30)),
                    )?;
                    if as_json {
                        out(&v);
                    } else if *notes {
                        if v["timed_out"] == true {
                            println!("no note within {}", timeout.as_deref().unwrap_or("60s"));
                        } else {
                            print_notes(&v["notes"], st);
                        }
                        for r in v["reviews"].as_array().into_iter().flatten() {
                            print_review(r, st);
                        }
                    } else {
                        print_status(&v, st, v["waited_ms"].as_f64());
                        for f in v["held"].as_array().into_iter().flatten() {
                            println!(
                                "{}",
                                st.yellow(&format!(
                                    "note: {} is open in an editor with other text: your change on disk is built once the editor takes it",
                                    f.as_str().unwrap_or("")
                                ))
                            );
                        }
                    }
                    if v["timed_out"] == true {
                        if !as_json && !*notes {
                            ui.error("timed out: no build has read the files as they are now");
                        }
                        return Ok(EXIT_TIMEOUT);
                    }
                    Ok(if *notes { 0 } else { build_code(&v) })
                }
                Verb::Problems {
                    errors,
                    warnings,
                    file,
                    context,
                    summary,
                } => {
                    let v = c.call(
                        "phitex/session/problems",
                        &json!({"errors": errors, "warnings": warnings, "file": file}),
                        None,
                    )?;
                    if as_json && *summary {
                        let list = v["problems"].as_array().cloned().unwrap_or_default();
                        out(&json!({"build": v["build"], "summary": summary_json(&list)}));
                    } else if as_json {
                        out(&v);
                    } else {
                        let root = PathBuf::from(s["root"].as_str().unwrap_or("."));
                        let list = v["problems"].as_array().cloned().unwrap_or_default();
                        if list.is_empty() {
                            println!("no problems (build {})", v["build"]);
                        } else if *summary {
                            print_summary(&list);
                        }
                        for p in list.iter().filter(|_| !*summary) {
                            println!("{}", problem_line(p, st));
                            if let Some(n) = context {
                                print_context(&root, p, *n, st);
                            }
                        }
                    }
                    let errs = v["problems"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|p| p["severity"] == "error"));
                    Ok(i32::from(errs))
                }
                Verb::Rebuild { clean } => {
                    let v = c.call("phitex/session/rebuild", &json!({"clean": clean}), None)?;
                    if as_json {
                        out(&v);
                    } else {
                        print_status(&v, st, None);
                    }
                    Ok(build_code(&v))
                }
                Verb::Log { tail, build } => {
                    let v = c.call("phitex/session/log", &json!({"build": build}), None)?;
                    if as_json {
                        out(&v);
                    } else {
                        let text = v["text"].as_str().unwrap_or("");
                        let lines: Vec<&str> = text.lines().collect();
                        let from = tail.map_or(0, |n| lines.len().saturating_sub(n));
                        for l in &lines[from..] {
                            println!("{l}");
                        }
                    }
                    Ok(0)
                }
                Verb::Pdf { output, synctex } => {
                    // (PATH from here, not from the watch's folder)
                    let to = output.as_ref().map(|o| {
                        std::env::current_dir()
                            .unwrap_or_default()
                            .join(o)
                            .display()
                            .to_string()
                    });
                    let v = c.call(
                        "phitex/session/pdf",
                        &json!({"to": to, "synctex": synctex}),
                        Some(Duration::from_secs(600)),
                    )?;
                    if as_json {
                        out(&v);
                    } else {
                        let pdf = v["pdf"].as_str().unwrap_or("");
                        println!("{pdf}");
                        if v["synctex"] == true {
                            println!("{}.synctex.gz", pdf.trim_end_matches(".pdf"));
                        }
                    }
                    Ok(0)
                }
                Verb::Stop => {
                    let pid = s["pid"].as_u64().unwrap_or(0);
                    c.call("phitex/session/stop", &json!({}), None)?;
                    // (until its process is gone, the exact PDF written
                    // as it ends)
                    drop(c);
                    let t0 = Instant::now();
                    while crate::attach::alive(&s) {
                        if t0.elapsed() > Duration::from_secs(120) {
                            return Err(Failed(
                                EXIT_TIMEOUT,
                                format!("the watch (pid {pid}) did not end"),
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    if as_json {
                        out(&json!({"stopped": pid}));
                    } else {
                        println!("stopped the watch (pid {pid})");
                    }
                    Ok(0)
                }
                Verb::Reply { id, text } => {
                    let v = c.call("phitex/notes/reply", &json!({"id": id, "text": text}), None)?;
                    if as_json {
                        out(&v);
                    } else {
                        println!("replied to {id}");
                    }
                    Ok(0)
                }
                Verb::Resolve { id, text } => {
                    let v = c.call(
                        "phitex/notes/resolve",
                        &json!({"id": id, "text": text}),
                        None,
                    )?;
                    if as_json {
                        out(&v);
                    } else {
                        let edited: Vec<String> = v["thread"]
                            .as_array()
                            .and_then(|t| t.last())
                            .and_then(|p| p["edited"].as_array())
                            .into_iter()
                            .flatten()
                            .map(edited_text)
                            .collect();
                        if edited.is_empty() {
                            println!("resolved {id}");
                        } else {
                            println!("resolved {id} (edited: {})", edited.join(", "));
                        }
                    }
                    Ok(0)
                }
                Verb::Changes => {
                    let v = c.call("phitex/changes/list", &json!({}), None)?;
                    if as_json {
                        out(&v);
                    } else {
                        print_changes(&v, st);
                    }
                    Ok(0)
                }
                Verb::List | Verb::Start { .. } | Verb::Notes { .. } => unreachable!(),
            }
        }
    }
}

/// The project's folder for `here`: the nearest above it with `.phitex/`
/// (its notes), else `here`.
fn project_root(here: &Path) -> PathBuf {
    here.ancestors()
        .find(|d| d.join(".phitex").join("notes.json").exists())
        .map_or_else(|| here.to_path_buf(), Path::to_path_buf)
}

fn build_code(v: &Value) -> i32 {
    i32::from(v["ok"] == false)
}

fn problem_line(p: &Value, st: Style) -> String {
    let file = p["file"].as_str().unwrap_or("").trim_start_matches("./");
    let place = match (file, p["line"].as_u64()) {
        ("", _) => String::new(),
        (f, Some(l)) => format!("{f}:{l}: "),
        (f, None) => format!("{f}: "),
    };
    let msg = p["message"].as_str().unwrap_or("");
    let sev = p["severity"].as_str().unwrap_or("");
    let msg = match sev {
        "error" => st.red(msg),
        "warning" => st.yellow(msg),
        _ => msg.to_owned(),
    };
    let mut out = format!("{place}{msg}");
    // (an error's help: what TeX or PhiTeX says to do, the engine to
    // name for a package's engine check)
    if sev == "error" {
        for h in p["help"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            out.push_str(&format!("\n  {}", st.dim(h)));
        }
    }
    out
}

/// The engine and the output folder the watch runs, and how each was
/// chosen: `engine xelatex (picked: the document loads fontspec) · output
/// folder . (the default: beside the main file)`.
fn setup_line(v: &Value) -> Option<String> {
    let engine = v["engine"].as_str()?;
    let mut s = format!("engine {engine}");
    if let Some(w) = v["engine_why"].as_str() {
        s.push_str(&format!(" ({w})"));
    }
    if let Some(d) = v["out_dir"].as_str() {
        s.push_str(&format!(" · output folder {d}"));
        if let Some(w) = v["out_dir_why"].as_str() {
            s.push_str(&format!(" ({w})"));
        }
    }
    Some(s)
}

/// `problems --summary`: the problems counted by kind, each kind's files
/// by count, most first: `overfull 211, of which ledger.tex 190, intro.tex 21`.
fn print_summary(list: &[Value]) {
    for l in summary(list) {
        println!("{l}");
    }
}

/// The problems counted by kind, most first, each kind's files by count.
fn tally(list: &[Value]) -> Vec<(String, usize, Vec<(String, usize)>)> {
    let mut kinds: Vec<(String, usize, Vec<(String, usize)>)> = Vec::new();
    for p in list {
        let kind = p["kind"].as_str().unwrap_or_else(|| {
            phitex_live::problems::kind_of(
                p["severity"].as_str().unwrap_or(""),
                p["message"].as_str().unwrap_or(""),
            )
        });
        let kind = kind.to_owned();
        let file = p["file"].as_str().unwrap_or("").trim_start_matches("./");
        let file = if file.is_empty() { "(no file)" } else { file };
        let k = kinds
            .iter()
            .position(|(k, ..)| *k == kind)
            .unwrap_or_else(|| {
                kinds.push((kind, 0, Vec::new()));
                kinds.len() - 1
            });
        kinds[k].1 += 1;
        match kinds[k].2.iter_mut().find(|(f, _)| f == file) {
            Some(f) => f.1 += 1,
            None => kinds[k].2.push((file.to_owned(), 1)),
        }
    }
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for k in &mut kinds {
        k.2.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
    kinds
}

/// `--summary --json`'s: `[{kind, count, files: [{file, count}]}]`.
fn summary_json(list: &[Value]) -> Value {
    tally(list)
        .into_iter()
        .map(|(kind, n, files)| {
            let files: Vec<Value> = files
                .into_iter()
                .map(|(f, c)| json!({"file": f, "count": c}))
                .collect();
            json!({"kind": kind, "count": n, "files": files})
        })
        .collect()
}

fn summary(list: &[Value]) -> Vec<String> {
    tally(list)
        .into_iter()
        .map(|(kind, n, files)| {
            let shown: Vec<String> = files
                .iter()
                .take(5)
                .map(|(f, c)| format!("{f} {c}"))
                .collect();
            let more = files.len().saturating_sub(5);
            let tail = if more > 0 {
                format!(", {more} more files")
            } else {
                String::new()
            };
            format!("{kind} {n}, of which {}{tail}", shown.join(", "))
        })
        .collect()
}

fn print_context(root: &Path, p: &Value, n: usize, st: Style) {
    let (Some(file), Some(line)) = (p["file"].as_str(), p["line"].as_u64()) else {
        return;
    };
    let Ok(text) = std::fs::read(root.join(file.trim_start_matches("./"))) else {
        return;
    };
    let text = String::from_utf8_lossy(&text);
    let line = usize::try_from(line).unwrap_or(1);
    let lines: Vec<&str> = text.lines().collect();
    let from = line.saturating_sub(n + 1);
    let to = (line + n).min(lines.len());
    for (i, l) in lines.iter().enumerate().take(to).skip(from) {
        let k = i + 1;
        let mark = if k == line { ">" } else { " " };
        println!("  {mark}{} {l}", st.dim(&format!("{k:>5} |")));
    }
}

fn print_status(v: &Value, st: Style, waited: Option<f64>) {
    let Some(b) = v["build"].as_u64() else {
        println!("no build yet");
        return;
    };
    let ok = v["ok"] != false;
    let mut parts = vec![format!("build {b}")];
    if ok {
        parts.push(st.green("ok"));
    } else {
        let n = v["errors"].as_u64().unwrap_or(0);
        parts.push(st.red(&format!("{n} error{}", if n == 1 { "" } else { "s" })));
    }
    if let Some(p) = v["pages"].as_u64() {
        parts.push(format!("{p} page{}", if p == 1 { "" } else { "s" }));
    }
    if let Some(ms) = v["ms"].as_f64() {
        parts.push(crate::ui::ms(Duration::from_secs_f64(ms / 1e3)));
    }
    let w = v["warnings"].as_u64().unwrap_or(0);
    if v["diagnostics_pending"] == true {
        parts.push("warnings not made yet (--warnings waits for them)".into());
    } else if w > 0 {
        parts.push(format!("{w} warning{}", if w == 1 { "" } else { "s" }));
    }
    if let Some(ms) = waited {
        parts.push(st.dim(&format!(
            "waited {}",
            crate::ui::ms(Duration::from_secs_f64(ms / 1e3))
        )));
    }
    println!("{}", parts.join(" · "));
    for p in v["problems"].as_array().into_iter().flatten() {
        println!("{}", problem_line(p, st));
    }
}

fn edited_text(e: &Value) -> String {
    let f = e["file"].as_str().unwrap_or("");
    let (a, b) = (
        e["from_line"].as_u64().unwrap_or(0),
        e["to_line"].as_u64().unwrap_or(0),
    );
    if a == b {
        format!("{f}:{a}")
    } else {
        format!("{f}:{a}–{b}")
    }
}

/// The last `max` characters of `s` on one line and a space, "…" before
/// them where cut.
fn tail(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = s.chars().count();
    if n == 0 {
        return String::new();
    }
    if n <= max {
        return format!("{s} ");
    }
    format!("…{} ", s.chars().skip(n - max).collect::<String>())
}

/// A quote on one line, cut to `max` characters.
fn one_line(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::ui::clip(&s, max)
}

fn print_notes(notes: &Value, st: Style) {
    let list = notes.as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        println!("no notes");
        return;
    }
    for n in &list {
        let a = &n["anchor"];
        let mut head = format!(
            "{}  {}  {}:{}",
            st.bold(n["id"].as_str().unwrap_or("")),
            n["kind"].as_str().unwrap_or(""),
            a["file"].as_str().unwrap_or(""),
            a["line"]
        );
        if n["status"] == "resolved" {
            head.push_str(&st.dim("  resolved"));
        }
        if n["detached"] == true {
            head.push_str(&st.yellow("  detached (its text is gone)"));
        }
        if let Some(h) = n.get("hunk").filter(|h| h.is_object()) {
            head.push_str(&st.dim(&format!(
                "  on your change at {}:{}",
                h["file"].as_str().unwrap_or(""),
                h["line"]
            )));
        }
        println!("{head}");
        let quote = a["quote"].as_str().unwrap_or("");
        let on_change = n.get("hunk").is_some_and(Value::is_object);
        if quote.is_empty() && on_change {
            // (a comment on a change: the change says where)
        } else if quote.is_empty() {
            println!("    {}", st.dim("(a spot, no text)"));
        } else {
            println!(
                "    {}\u{201c}{}\u{201d}{}",
                st.dim(&tail(a["before"].as_str().unwrap_or(""), 30)),
                one_line(quote, 200),
                st.dim(&format!(
                    " {}",
                    one_line(a["after"].as_str().unwrap_or(""), 30)
                ))
            );
        }
        if let Some(h) = n.get("hunk").filter(|h| h.is_object()) {
            println!(
                "    {} {}",
                st.red(&format!(
                    "-{}",
                    one_line(h["old"].as_str().unwrap_or(""), 100)
                )),
                st.green(&format!(
                    "+{}",
                    one_line(h["new"].as_str().unwrap_or(""), 100)
                ))
            );
        }
        for p in n["thread"].as_array().into_iter().flatten() {
            let by = p["by"].as_str().unwrap_or("");
            if let Some(d) = p.get("diff").filter(|d| d.is_object()) {
                println!(
                    "    {} {} {}",
                    st.dim("updated:"),
                    st.red(&format!(
                        "-{}",
                        one_line(d["old"].as_str().unwrap_or(""), 80)
                    )),
                    st.green(&format!(
                        "+{}",
                        one_line(d["new"].as_str().unwrap_or(""), 80)
                    ))
                );
                continue;
            }
            let edited: Vec<String> = p["edited"]
                .as_array()
                .into_iter()
                .flatten()
                .map(edited_text)
                .collect();
            let mut line = format!("    {by}: {}", p["text"].as_str().unwrap_or(""));
            if !edited.is_empty() {
                line.push_str(&st.dim(&format!(" (edited: {})", edited.join(", "))));
            }
            println!("{line}");
        }
    }
}

fn print_review(r: &Value, st: Style) {
    let verdict = match r["verdict"].as_str().unwrap_or("") {
        "approve" => st.green("approved"),
        "request_changes" => st.yellow("changes requested"),
        "reject" => st.red("rejected"),
        _ => "commented".to_owned(),
    };
    let s = r["summary"].as_str().unwrap_or("");
    println!(
        "review {}: {verdict}{}",
        r["id"].as_str().unwrap_or(""),
        if s.is_empty() {
            String::new()
        } else {
            format!(": {s}")
        }
    );
}

fn print_changes(v: &Value, st: Style) {
    if v["off"] == true {
        println!(
            "{}",
            v["how"].as_str().unwrap_or("reviewing your edits is off")
        );
        return;
    }
    let pending = v["pending"].as_array().cloned().unwrap_or_default();
    if pending.is_empty() {
        println!("no changes waiting for the user");
    } else {
        println!("waiting for the user ({}):", pending.len());
    }
    for h in &pending {
        let what = match h["whole"].as_str() {
            Some("added") => " (new file)".to_owned(),
            Some("deleted") => " (file deleted)".to_owned(),
            _ => String::new(),
        };
        let note = h["note"]
            .as_str()
            .map(|n| st.dim(&format!("  [{n}]")))
            .unwrap_or_default();
        println!(
            "  {}:{}{what}  {} {}{note}",
            h["file"].as_str().unwrap_or(""),
            h["line"],
            st.red(&format!(
                "-{}",
                one_line(h["old"].as_str().unwrap_or(""), 60)
            )),
            st.green(&format!(
                "+{}",
                one_line(h["new"].as_str().unwrap_or(""), 60)
            ))
        );
    }
    let verdicts = v["verdicts"].as_array().cloned().unwrap_or_default();
    if !verdicts.is_empty() {
        println!("the user's verdicts since you last looked:");
    }
    // (the accepted counted by file; each rejected one said with its text)
    let mut accepted: Vec<(String, usize)> = Vec::new();
    for x in verdicts.iter().filter(|x| x["verdict"] == "accepted") {
        let f = x["hunk"]["file"].as_str().unwrap_or("").to_owned();
        match accepted.iter_mut().find(|(g, _)| *g == f) {
            Some((_, n)) => *n += 1,
            None => accepted.push((f, 1)),
        }
    }
    if !accepted.is_empty() {
        let n: usize = accepted.iter().map(|x| x.1).sum();
        let by: Vec<String> = accepted.iter().map(|(f, k)| format!("{f} {k}")).collect();
        println!(
            "  {} {n} change{} ({})",
            st.green("accepted"),
            if n == 1 { "" } else { "s" },
            by.join(", ")
        );
    }
    for x in verdicts.iter().filter(|x| x["verdict"] == "rejected") {
        let h = &x["hunk"];
        println!(
            "  {} {}:{} (kept {:?}, not {:?}: do not write it again)",
            st.red("rejected"),
            h["file"].as_str().unwrap_or(""),
            h["line"],
            one_line(h["old"].as_str().unwrap_or(""), 60),
            one_line(h["new"].as_str().unwrap_or(""), 60)
        );
    }
    let comments = v["comments"].as_array().cloned().unwrap_or_default();
    if !comments.is_empty() {
        println!("comments on your changes:");
        print_notes(&Value::Array(comments), st);
    }
    for r in v["reviews"].as_array().into_iter().flatten().rev().take(3) {
        print_review(r, st);
    }
}

/// `start`: the watch for FILE found or started, then its first build.
fn start(
    file: Option<&str>,
    timeout: Option<&str>,
    args: &[String],
    as_json: bool,
    st: Style,
) -> Result<i32, Failed> {
    let t = match timeout {
        Some(t) => duration(t).map_err(|e| Failed(EXIT_REFUSED, e))?,
        None => Some(Duration::from_secs(600)),
    };
    let t0 = Instant::now();
    let (v, started) = match find(None, file) {
        Ok(v) => (v, false),
        Err(_) => (launch(file, args, t)?, true),
    };
    let left = t.map(|d| d.saturating_sub(t0.elapsed()));
    let mut c = Conn::open(&v)?;
    let ms = left.map_or(0, |d| d.as_millis().max(1));
    let w = c.call(
        "phitex/session/wait",
        &json!({"timeout_ms": ms}),
        left.map(|d| d + Duration::from_secs(30)),
    )?;
    let log = PathBuf::from(v["root"].as_str().unwrap_or("."))
        .join(".phitex")
        .join("session.log");
    if as_json {
        println!(
            "{}",
            json!({"pid": v["pid"], "viewer": w["viewer"], "main": v["main"], "root": v["root"],
                   "log": log.display().to_string(), "started": started, "status": w})
        );
    } else {
        println!(
            "{} pid {} · viewer {} · log {}",
            if started { "started" } else { "running" },
            v["pid"],
            w["viewer"]
                .as_str()
                .or(v["viewer"].as_str())
                .unwrap_or("(none: --no-view)"),
            log.display()
        );
        if let Some(l) = setup_line(&w) {
            println!("{l}");
        }
        print_status(&w, st, None);
    }
    if w["timed_out"] == true {
        return Ok(EXIT_TIMEOUT);
    }
    Ok(build_code(&w))
}

/// A watch started detached: its discovery file once it is there.
fn launch(file: Option<&str>, args: &[String], t: Option<Duration>) -> Result<Value, Failed> {
    let here = std::env::current_dir().map_err(|e| Failed(EXIT_REFUSED, e.to_string()))?;
    let dir = here.join(".phitex");
    if let Some(said) = crate::review::make_dir(&here).map_err(|e| Failed(EXIT_REFUSED, e))? {
        eprintln!("phitex: {said}");
    }
    let log_path = dir.join("session.log");
    let log = std::fs::File::create(&log_path)
        .map_err(|e| Failed(EXIT_REFUSED, format!("{}: {e}", log_path.display())))?;
    let err = log
        .try_clone()
        .map_err(|e| Failed(EXIT_REFUSED, e.to_string()))?;
    let sessions =
        crate::attach::dir().map_err(|e| no_session(format!("the sessions folder: {e}")))?;
    // (the folder watched before the watch starts: its file seen when made)
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |e| {
        let _ = tx.send(e);
    })
    .map_err(|e| {
        Failed(
            EXIT_REFUSED,
            format!("watching {}: {e}", sessions.display()),
        )
    })?;
    notify::Watcher::watch(&mut watcher, &sessions, notify::RecursiveMode::NonRecursive).map_err(
        |e| {
            Failed(
                EXIT_REFUSED,
                format!("watching {}: {e}", sessions.display()),
            )
        },
    )?;
    let exe = std::env::current_exe().map_err(|e| Failed(EXIT_REFUSED, e.to_string()))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("watch").arg("--write");
    cmd.args(args);
    if let Some(f) = file {
        cmd.arg(f);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(err)
        .env("NO_COLOR", "1");
    // (its own process group: the agent's Ctrl+C does not end it)
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd
        .spawn()
        .map_err(|e| Failed(EXIT_REFUSED, format!("phitex watch: {e}")))?;
    let pid = child.id();
    let mine = sessions.join(format!("{pid}.json"));
    let t0 = Instant::now();
    loop {
        if mine.exists()
            && let Ok(b) = std::fs::read(&mine)
            && let Ok(v) = serde_json::from_slice::<Value>(&b)
            && v["main"].is_string()
        {
            return Ok(v);
        }
        if let Ok(Some(s)) = child.try_wait() {
            let tail = std::fs::read_to_string(&log_path).unwrap_or_default();
            let tail: Vec<&str> = tail.lines().rev().take(8).collect();
            return Err(no_session(format!(
                "the watch ended ({s}):\n{}",
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            )));
        }
        let left = t.map_or(Duration::from_secs(1), |d| d.saturating_sub(t0.elapsed()));
        if left.is_zero() {
            return Err(Failed(
                EXIT_TIMEOUT,
                "the watch did not start in time".into(),
            ));
        }
        // (an event of the folder, or a second: the child's end is looked at too)
        let _ = rx.recv_timeout(left.min(Duration::from_secs(1)));
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn problems_counted_by_kind_and_file() {
        let p = |sev: &str, m: &str, f: &str| json!({"severity": sev, "message": m, "file": f});
        let mut list = vec![p("error", "Undefined control sequence", "./main.tex")];
        list.extend((0..3).map(|_| {
            p(
                "warning",
                "Overfull \\hbox (1.0pt too wide) in paragraph",
                "ledger.tex",
            )
        }));
        list.push(p(
            "warning",
            "Overfull \\hbox (2.0pt too wide) in paragraph",
            "intro.tex",
        ));
        list.push(json!({"severity": "warning", "message": "Citation `x' undefined", "file": "main.tex", "kind": "reference"}));
        assert_eq!(
            summary(&list),
            [
                "overfull 4, of which ledger.tex 3, intro.tex 1",
                "error 1, of which main.tex 1",
                "reference 1, of which main.tex 1",
            ]
        );
        assert_eq!(
            summary_json(&list)[0],
            json!({"kind": "overfull", "count": 4, "files": [{"file": "ledger.tex", "count": 3}, {"file": "intro.tex", "count": 1}]})
        );
    }

    use super::*;

    #[test]
    fn times() {
        assert_eq!(duration("60s").unwrap(), Some(Duration::from_secs(60)));
        assert_eq!(duration("10m").unwrap(), Some(Duration::from_secs(600)));
        assert_eq!(duration("500ms").unwrap(), Some(Duration::from_millis(500)));
        assert_eq!(duration("2").unwrap(), Some(Duration::from_secs(2)));
        assert_eq!(duration("0").unwrap(), None);
        assert!(duration("soon").is_err());
    }
}
