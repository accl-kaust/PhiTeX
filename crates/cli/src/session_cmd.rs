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
        /// …and for the exact PDF written after it
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
fn find(pid: Option<u32>, file: Option<&str>) -> Result<Value, Failed> {
    let d = crate::attach::dir().map_err(|e| no_session(format!("the sessions folder: {e}")))?;
    let live = crate::attach::live_in(&d);
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
    mine.pop().ok_or_else(|| {
        no_session("no phitex watch runs here: start one with `phitex session start`")
    })
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
pub fn main(verb: &Verb, pid: Option<u32>, as_json: bool, ui: &Ui) -> i32 {
    let st = Style::stdout();
    match run(verb, pid, as_json, st, ui) {
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

fn run(verb: &Verb, pid: Option<u32>, as_json: bool, st: Style, ui: &Ui) -> Result<i32, Failed> {
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
                } => {
                    let v = c.call(
                        "phitex/session/problems",
                        &json!({"errors": errors, "warnings": warnings, "file": file}),
                        None,
                    )?;
                    if as_json {
                        out(&v);
                    } else {
                        let root = PathBuf::from(s["root"].as_str().unwrap_or("."));
                        let list = v["problems"].as_array().cloned().unwrap_or_default();
                        if list.is_empty() {
                            println!("no problems (build {})", v["build"]);
                        }
                        for p in &list {
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
                Verb::Stop => {
                    let pid = s["pid"].as_u64().unwrap_or(0);
                    c.call("phitex/session/stop", &json!({}), None)?;
                    // (until its process is gone, the exact PDF written:
                    // its socket closes as it ends)
                    let _ =
                        c.r.get_ref()
                            .set_read_timeout(Some(Duration::from_secs(120)));
                    while crate::lsp::read(&mut c.r).is_some() {}
                    if crate::attach::alive(&s) {
                        return Err(Failed(
                            EXIT_TIMEOUT,
                            format!("the watch (pid {pid}) did not end"),
                        ));
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
    format!("{place}{msg}")
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
        if quote.is_empty() {
            println!("    {}", st.dim("(a spot, no text)"));
        } else {
            println!(
                "    {}\u{201c}{}\u{201d}{}",
                st.dim(
                    &one_line(a["before"].as_str().unwrap_or(""), 30)
                        .chars()
                        .rev()
                        .take(30)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>()
                ),
                one_line(quote, 200),
                st.dim(&one_line(a["after"].as_str().unwrap_or(""), 30))
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
    for x in &verdicts {
        let h = &x["hunk"];
        let how = if x["verdict"] == "rejected" {
            st.red("rejected")
        } else {
            st.green("accepted")
        };
        let detail = if x["verdict"] == "rejected" {
            format!(
                " (kept {:?}, not {:?}: do not write it again)",
                one_line(h["old"].as_str().unwrap_or(""), 60),
                one_line(h["new"].as_str().unwrap_or(""), 60)
            )
        } else {
            String::new()
        };
        println!(
            "  {how} {}:{}{detail}",
            h["file"].as_str().unwrap_or(""),
            h["line"]
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
    std::fs::create_dir_all(&dir)
        .map_err(|e| Failed(EXIT_REFUSED, format!("{}: {e}", dir.display())))?;
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
