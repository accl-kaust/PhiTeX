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
        /// How your changes are reviewed: direct (you edit the files, they
        /// wait as hunks) or propose (kept apart until the user accepts;
        /// write them in the workspace, then `session propose`). Default:
        /// `review-mode`, else direct
        #[arg(long, value_name = "MODE")]
        review: Option<String>,
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
    /// Who last changed what the pages show: each printed line with the
    /// git commit that last changed its source
    ///
    /// From the project's git history and the pages' glyph origins (the
    /// viewer's Blame); a line with text not committed yet says so. No
    /// build: the pages as they are.
    Blame {
        /// Only page N (from 1; repeat for more; default every page)
        #[arg(long = "page", value_name = "N")]
        pages: Vec<usize>,
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
    /// Send your change for review, with why (and a title)
    ///
    /// propose mode: what you wrote in the workspace (`status` names it;
    /// else the files you edited, put back as they were) becomes a
    /// proposal, the user's files untouched until they accept it; with
    /// --revise ID, a new revision of that proposal (its thread kept).
    /// direct mode: the changes waiting get the title and the why.
    Propose {
        /// Why: what the change does and for what (its first line the
        /// title unless --title)
        #[arg(short = 'm', long = "message", value_name = "WHY")]
        why: String,
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// A new revision of proposal ID
        #[arg(long, value_name = "ID")]
        revise: Option<String>,
    },
    /// The review mode: direct or propose (none: say it)
    Mode {
        #[arg(value_name = "MODE")]
        mode: Option<String>,
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
    /// Draw what a label labels (a figure or table with its caption, a
    /// subfigure, an equation, a heading), or what a source line made,
    /// cropped, as a PNG: its path, page and box
    ///
    /// Waits for a build of the files as they are now (as `wait`), then
    /// draws from its exact PDF; no other program is run. What a label
    /// labels is the innermost environment around its \label (else the
    /// paragraph or heading it ends). It is found on the page the engine
    /// placed it on by its words; the part is their lines, what the page
    /// draws among them, and the pictures and rules beside them that the
    /// source puts there. Written to .phitex/png/LABEL-buildN.png
    /// (FILE-LINE-buildN.png) unless --out. The watch's glyph origins (a
    /// watch with its viewer, as `session start`'s, or with --synctex)
    /// name the page at once and tell the text around it apart; without
    /// them every page is looked at.
    Png {
        /// The label (as in \label{LABEL}) of what to draw
        #[arg(
            long,
            value_name = "LABEL",
            required_unless_present_any = ["at", "change"],
            conflicts_with_all = ["at", "change"]
        )]
        label: Option<String>,
        /// What line LINE (from 1) of FILE made, instead
        #[arg(long, value_name = "FILE:LINE", conflicts_with = "change")]
        at: Option<String>,
        /// Draw from the diff against REV instead (as `session compare
        /// REV` makes it): the label's or the line's part of its pages,
        /// or change N's
        #[arg(long, value_name = "REV")]
        compare: Option<String>,
        /// With --compare: change N (from 1, as `session compare` numbers
        /// them)
        #[arg(long, value_name = "N", requires = "compare")]
        change: Option<usize>,
        /// Pixels per inch
        #[arg(long, value_name = "N", default_value_t = 150)]
        dpi: u32,
        /// Write the PNG to PATH
        #[arg(long, value_name = "PATH")]
        out: Option<String>,
        /// Points of the page around it (default 3)
        #[arg(long, value_name = "PT")]
        context: Option<String>,
        /// The same part of the page in the build before and in this one,
        /// side by side, then this one with the pixels that changed in red
        /// (.phitex/png/NAME-buildM-vs-N.png)
        #[arg(long)]
        diff: bool,
        /// Give up waiting for the build after TIME (default 60s)
        #[arg(long, value_name = "TIME")]
        timeout: Option<String>,
    },
    /// Diff the files as they are against an older version (latexdiff's
    /// markup, as the viewer's Compare), and say where each change is
    ///
    /// REV is a git commit, branch or HEAD~N, or the path of an older copy
    /// of the main file. Waits for the diff's build (started from the
    /// watch's own: about an edit's time). Prints the diff PDF's path and
    /// the changes, one a line: its number, page, box (points from the
    /// page's top left) and file:line. The diff is the agent's own: the
    /// viewer's Compare is left as it is. Again with no REV: the last one,
    /// diffed again against the files as they are now.
    Compare {
        /// The version to compare with
        #[arg(value_name = "REV", required_unless_present = "stop")]
        rev: Option<String>,
        /// End the agent's diff
        #[arg(long, conflicts_with = "rev")]
        stop: bool,
        /// The look, as the viewer's diff settings: markup=underline|
        /// ctraditional|traditional|cfont|fontstrike|bold|invisible (or a
        /// latexdiff type with change bars: culinechbar, cfontchbar,
        /// cchangebar, changebar), subtype=safe|color|marker, add=#RRGGBB,
        /// del=#RRGGBB; comma-separated
        #[arg(long, value_name = "KEY=VALUE,...")]
        style: Option<String>,
    },
    /// Where what a label labels (or what a source line made) is: its
    /// page and box in points from the page's top left, nothing drawn
    ///
    /// As `png` finds it.
    Where {
        /// The label (as in \label{LABEL})
        #[arg(
            value_name = "LABEL",
            required_unless_present = "at",
            conflicts_with = "at"
        )]
        label: Option<String>,
        /// What line LINE (from 1) of FILE made, instead
        #[arg(long, value_name = "FILE:LINE")]
        at: Option<String>,
        /// Points of the page around it (default 3)
        #[arg(long, value_name = "PT")]
        context: Option<String>,
        /// Give up waiting for the build after TIME (default 60s)
        #[arg(long, value_name = "TIME")]
        timeout: Option<String>,
    },
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
            review,
            args,
        } => start(
            file.as_deref(),
            timeout.as_deref(),
            review.as_deref(),
            args,
            as_json,
            st,
        ),
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
                        print_proposals(&v, st);
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
                Verb::Blame { pages } => {
                    let ks: Vec<usize> = pages.iter().map(|k| k.saturating_sub(1)).collect();
                    let v = c.call(
                        "phitex/session/blame",
                        &json!({"pages": ks}),
                        Some(Duration::from_secs(300)),
                    )?;
                    if as_json {
                        out(&v);
                    } else {
                        print_blame(&v, st);
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
                Verb::Png {
                    label,
                    at,
                    compare: Some(rev),
                    change,
                    dpi,
                    out: to,
                    context,
                    diff: _,
                    timeout: _,
                } => {
                    let root = PathBuf::from(s["root"].as_str().unwrap_or("."));
                    let (f, what) = compare_part(
                        &mut c,
                        &root,
                        rev,
                        label.as_deref(),
                        at.as_deref(),
                        *change,
                        context.as_deref(),
                    )?;
                    let v = draw(&f, &what, *dpi, to.as_deref(), false)?;
                    if as_json {
                        out(&v);
                    } else {
                        println!("{}", where_line(&f, &what));
                        println!(
                            "{} ({}x{} px, {dpi} dpi)",
                            v["png"].as_str().unwrap_or(""),
                            v["width"],
                            v["height"]
                        );
                    }
                    Ok(0)
                }
                Verb::Png {
                    label,
                    at,
                    compare: None,
                    change: _,
                    dpi,
                    out: to,
                    context,
                    diff,
                    timeout,
                } => {
                    let what = What::of(label.as_deref(), at.as_deref())?;
                    let f = locate(&mut c, &s, &what, context.as_deref(), timeout.as_deref())?;
                    let v = draw(&f, &what, *dpi, to.as_deref(), *diff)?;
                    if as_json {
                        out(&v);
                    } else {
                        println!("{}", where_line(&f, &what));
                        println!(
                            "{} ({}x{} px, {dpi} dpi)",
                            v["png"].as_str().unwrap_or(""),
                            v["width"],
                            v["height"]
                        );
                        if *diff {
                            let n = v["changed"].as_u64().unwrap_or(0);
                            println!(
                                "{}",
                                if n == 0 {
                                    format!(
                                        "build {} and build {} draw it alike",
                                        v["before"], f.build
                                    )
                                } else {
                                    format!(
                                        "{n} pixels differ between build {} (left) and build {} (middle; right: in red)",
                                        v["before"], f.build
                                    )
                                }
                            );
                        }
                    }
                    Ok(0)
                }
                Verb::Compare { rev, stop, style } => {
                    let v = compare_call(&mut c, rev.as_deref(), *stop, style.as_deref(), None)?;
                    if as_json {
                        out(&v);
                    } else if *stop {
                        println!(
                            "{}",
                            if v["stopped"] == true {
                                "the agent's diff ended"
                            } else {
                                "no agent's diff was running"
                            }
                        );
                    } else {
                        print_compare(&v, st);
                    }
                    Ok(i32::from(
                        v["errors"].as_array().is_some_and(|e| !e.is_empty()),
                    ))
                }
                Verb::Where {
                    label,
                    at,
                    context,
                    timeout,
                } => {
                    let what = What::of(label.as_deref(), at.as_deref())?;
                    let f = locate(&mut c, &s, &what, context.as_deref(), timeout.as_deref())?;
                    if as_json {
                        out(&found_json(&f, &what));
                    } else {
                        println!("{}", where_line(&f, &what));
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
                Verb::Propose { why, title, revise } => {
                    let v = c.call(
                        "phitex/propose",
                        &json!({"why": why, "title": title, "revise": revise}),
                        None,
                    )?;
                    if as_json {
                        out(&v);
                    } else if v["mode"] == "direct" {
                        println!(
                            "{}: \"{}\", {} change{} waiting for the user",
                            v["id"].as_str().unwrap_or(""),
                            v["title"].as_str().unwrap_or(""),
                            v["hunks"].as_array().map_or(0, Vec::len),
                            if v["hunks"].as_array().map_or(0, Vec::len) == 1 {
                                ""
                            } else {
                                "s"
                            }
                        );
                    } else {
                        let n = v["hunks"].as_array().map_or(0, Vec::len);
                        println!(
                            "{} (revision {}): \"{}\", {n} change{} in {}, waiting for the user",
                            v["id"].as_str().unwrap_or(""),
                            v["revision"],
                            v["title"].as_str().unwrap_or(""),
                            if n == 1 { "" } else { "s" },
                            v["files"]
                                .as_array()
                                .map(|f| f
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", "))
                                .unwrap_or_default()
                        );
                        if v["from"] == "files" {
                            println!(
                                "the files you edited are back as they were; the change is in the proposal"
                            );
                        }
                        println!(
                            "workspace (as the files are now): {}",
                            v["workspace"].as_str().unwrap_or("")
                        );
                    }
                    Ok(0)
                }
                Verb::Mode { mode } => {
                    let v = c.call("phitex/review/mode", &json!({"mode": mode}), None)?;
                    if as_json {
                        out(&v);
                    } else {
                        print!("review mode: {}", v["mode"].as_str().unwrap_or(""));
                        match v["workspace"].as_str() {
                            Some(w) => println!(
                                " · write your changes in {w}, then `phitex session propose -m WHY`"
                            ),
                            None => println!(" · edit the files; your changes wait as hunks"),
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

/// `session compare`'s request: the diff against `rev` (none: the last
/// one), in the look `style` (`KEY=VALUE,...`), `at` (a file from the
/// project's folder and a line) placed on its pages too.
fn compare_call(
    c: &mut Conn,
    rev: Option<&str>,
    stop: bool,
    style: Option<&str>,
    at: Option<(&str, usize)>,
) -> Result<Value, Failed> {
    let mut p = json!({"rev": rev, "stop": stop});
    for kv in style.into_iter().flat_map(|s| s.split(',')) {
        let kv = kv.trim();
        if kv.is_empty() {
            continue;
        }
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| Failed(EXIT_REFUSED, format!("--style {kv}: KEY=VALUE")))?;
        let key = match k.trim() {
            "markup" | "type" => "markup",
            "subtype" | "mode" => "subtype",
            "add" | "add_color" | "added" => "add",
            "del" | "del_color" | "deleted" => "del",
            other => {
                return Err(Failed(
                    EXIT_REFUSED,
                    format!("--style: no key {other} (markup, subtype, add, del)"),
                ));
            }
        };
        let v = v.trim();
        let ok = match key {
            "markup" => phitex_diff::Markup::parse(v).is_some(),
            "subtype" => phitex_diff::Subtype::parse(v).is_some(),
            _ => phitex_diff::Color::parse(v).is_ok(),
        };
        if !ok {
            return Err(Failed(
                EXIT_REFUSED,
                format!("--style: {key}={v} is not one"),
            ));
        }
        p[key] = json!(v);
    }
    if let Some((f, l)) = at {
        p["at"] = json!([f, l]);
    }
    c.call(
        "phitex/session/compare",
        &p,
        Some(Duration::from_secs(3600)),
    )
}

/// `session compare`'s answer, a line each: the PDF, the changes.
fn print_compare(v: &Value, st: Style) {
    let changes = v["changes"].as_array().cloned().unwrap_or_default();
    println!(
        "{} ({} page{}): {} change{} against {}",
        v["pdf"].as_str().unwrap_or("(no PDF)"),
        v["pages"],
        if v["pages"] == 1 { "" } else { "s" },
        changes.len(),
        if changes.len() == 1 { "" } else { "s" },
        v["rev"].as_str().unwrap_or("")
    );
    for c in &changes {
        let place = match (c["page"].as_u64(), c["bbox"].as_array()) {
            (Some(p), Some(b)) => format!(
                "page {p}, {}",
                b.iter()
                    .map(|x| format!("{:.2}", x.as_f64().unwrap_or(0.0)))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            _ => "not shown".to_owned(),
        };
        let text = |k: &str| one_line(c[k].as_str().unwrap_or(""), 40);
        let what = match c["kind"].as_str().unwrap_or("") {
            "add" => format!("added \"{}\"", text("new_text")),
            "del" => format!("deleted \"{}\"", text("old_text")),
            _ => format!("\"{}\" -> \"{}\"", text("old_text"), text("new_text")),
        };
        println!(
            "{:>3}  {}:{}  {place}  {what}",
            c["n"],
            c["file"].as_str().unwrap_or(""),
            c["line"]
        );
    }
    for e in v["errors"].as_array().into_iter().flatten().take(3) {
        println!("{}", st.red(e.as_str().unwrap_or("")));
    }
}

/// `png --compare`'s part of the diff's pages: change `change`'s box, or
/// where the label's line (its `\label`'s line in the project's files) or
/// line `at` is shown.
fn compare_part(
    c: &mut Conn,
    root: &Path,
    rev: &str,
    label: Option<&str>,
    at: Option<&str>,
    change: Option<usize>,
    context: Option<&str>,
) -> Result<(Found, What), Failed> {
    let pad = match context {
        Some(p) => p
            .trim()
            .trim_end_matches("pt")
            .parse::<f64>()
            .map_err(|_| Failed(EXIT_REFUSED, format!("--context {p}: points")))?,
        None => 3.0,
    };
    let (what, line) = match (change, label, at) {
        (Some(n), _, _) => (What::Change(n), None),
        (None, Some(l), _) => {
            // (the label's line, in the project's .tex files)
            let mut found = None;
            for e in walk_tex(root) {
                let Ok(t) = std::fs::read(&e) else { continue };
                if let Some((a, _)) = crate::shot::label_place(&t, l) {
                    let n = t[..a].split(|&b| b == b'\n').count();
                    let rel = e.strip_prefix(root).unwrap_or(&e).display().to_string();
                    found = Some((rel, n));
                    break;
                }
            }
            let (f, n) = found.ok_or_else(|| {
                Failed(
                    EXIT_REFUSED,
                    format!("no \\label{{{l}}} in the project's files"),
                )
            })?;
            (What::Label(l.to_owned()), Some((f, n)))
        }
        (None, None, at) => match What::of(None, at)? {
            What::Line(f, n) => {
                // (from here, as the project's folder names it)
                let here = std::env::current_dir().unwrap_or_default();
                let abs = here.join(&f);
                let rel = abs
                    .canonicalize()
                    .ok()
                    .and_then(|a| {
                        let r = root.canonicalize().ok()?;
                        a.strip_prefix(r).ok().map(|p| p.display().to_string())
                    })
                    .unwrap_or(f.clone());
                (What::Line(f, n), Some((rel, n)))
            }
            w => (w, None),
        },
    };
    let v = compare_call(
        c,
        Some(rev),
        false,
        None,
        line.as_ref().map(|(f, n)| (f.as_str(), *n)),
    )?;
    let pdf = PathBuf::from(
        v["pdf"]
            .as_str()
            .ok_or_else(|| Failed(EXIT_REFUSED, "the diff made no PDF".to_owned()))?,
    );
    let place = match &what {
        What::Change(n) => v["changes"]
            .as_array()
            .and_then(|cs| cs.get(n.checked_sub(1)?))
            .ok_or_else(|| {
                Failed(
                    EXIT_REFUSED,
                    format!(
                        "no change {n}: the diff has {}",
                        v["changes"].as_array().map_or(0, Vec::len)
                    ),
                )
            })?
            .clone(),
        _ => v["at"].clone(),
    };
    let (Some(page), Some(b)) = (place["page"].as_u64(), place["bbox"].as_array()) else {
        return Err(Failed(
            EXIT_REFUSED,
            format!("{}: not shown on the diff's pages", what.said()),
        ));
    };
    let b: Vec<f64> = b.iter().filter_map(Value::as_f64).collect();
    let rect = [b[0], b[1], b[2], b[3]];
    let k = usize::try_from(page).unwrap_or(1).saturating_sub(1);
    // (padded on every side: the page's edge is not needed)
    let size = (rect[2] + pad, rect[3] + pad);
    Ok((
        Found {
            build: 0,
            page: k,
            rect: crate::shot::padded(rect, pad, size),
            pdf,
            before: None,
            number: None,
            printed: None,
            approximate: false,
        },
        what,
    ))
}

/// The `.tex` files under `root` (not in hidden folders), for a label.
fn walk_tex(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let hidden = p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "tex") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
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
    let mut s = String::new();
    if let Some(m) = v["review"].as_str() {
        s.push_str(&format!("review {m}"));
        if let Some(w) = v["workspace"].as_str() {
            s.push_str(&format!(
                " (write your changes in {w}, then `phitex session propose -m WHY`)"
            ));
        }
        s.push_str(" · ");
    }
    s.push_str(&format!("engine {engine}"));
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
            // (the user's words edited since written: read them again)
            if let Some(at) = p["edited_at"].as_str() {
                let hm = at.get(11..16).unwrap_or(at);
                line.push_str(&st.yellow(&format!(" (edited {hm} UTC)")));
            }
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

/// The proposals in `session changes`: each with its state, the user's
/// decisions since you last looked, its thread.
fn print_proposals(v: &Value, st: Style) {
    for x in v["proposals"].as_array().into_iter().flatten() {
        let hunks = x["hunks"].as_array().cloned().unwrap_or_default();
        let count = |s: &str| hunks.iter().filter(|h| h["state"] == s).count();
        let state = match x["status"].as_str().unwrap_or("") {
            "changes_requested" => st.yellow("changes requested"),
            "accepted" => st.green("accepted"),
            "rejected" => st.red("rejected"),
            "partial" => "partly accepted".to_owned(),
            o => o.to_owned(),
        };
        if x["mode"] == "direct" {
            println!(
                "{} \"{}\" ({state}): your changes above",
                x["id"].as_str().unwrap_or(""),
                x["title"].as_str().unwrap_or("")
            );
        } else {
            println!(
                "{} r{} \"{}\" ({state}): {} open, {} accepted, {} rejected{}",
                x["id"].as_str().unwrap_or(""),
                x["revision"],
                x["title"].as_str().unwrap_or(""),
                count("open"),
                count("accepted"),
                count("rejected"),
                match count("conflict") {
                    0 => String::new(),
                    n => format!(", {n} conflicting with the user's edits since"),
                }
            );
        }
        for d in x["decided"].as_array().into_iter().flatten() {
            let h = &d["hunk"];
            println!(
                "  {} {}:{}  {} {}",
                d["verdict"].as_str().unwrap_or(""),
                h["file"].as_str().unwrap_or(""),
                h["line"],
                st.red(&format!(
                    "-{}",
                    one_line(h["old"].as_str().unwrap_or(""), 50)
                )),
                st.green(&format!(
                    "+{}",
                    one_line(h["new"].as_str().unwrap_or(""), 50)
                ))
            );
        }
        for p in x["thread"].as_array().into_iter().flatten() {
            let on = p["diff"]["file"]
                .as_str()
                .map(|f| {
                    format!(
                        " (on {f}:{} -{} +{})",
                        p["diff"]["line"],
                        one_line(p["diff"]["old"].as_str().unwrap_or(""), 30),
                        one_line(p["diff"]["new"].as_str().unwrap_or(""), 30)
                    )
                })
                .unwrap_or_default();
            println!(
                "    {}: {}{on}",
                p["by"].as_str().unwrap_or(""),
                p["text"].as_str().unwrap_or("")
            );
            for c in p["changes"].as_array().into_iter().flatten() {
                println!(
                    "      updated {}:{}  -{} +{}",
                    c["file"].as_str().unwrap_or(""),
                    c["line"],
                    one_line(c["old"].as_str().unwrap_or(""), 40),
                    one_line(c["new"].as_str().unwrap_or(""), 40)
                );
            }
        }
    }
}

fn print_changes(v: &Value, st: Style) {
    if v["off"] == true {
        println!(
            "{}",
            v["how"].as_str().unwrap_or("reviewing your edits is off")
        );
        return;
    }
    print_proposals(v, st);
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
    review: Option<&str>,
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
    if let Some(m) = review {
        c.call("phitex/review/mode", &json!({"mode": m}), None)?;
    }
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

/// What `png` and `where` look for: a label, or a source line.
#[derive(Debug)]
enum What {
    Label(String),
    /// A file (as given, from here) and its line, from 1.
    Line(String, usize),
    /// A change of a diff, from 1 (`png --compare --change`).
    Change(usize),
}

impl What {
    fn of(label: Option<&str>, at: Option<&str>) -> Result<What, Failed> {
        if let Some(l) = label {
            return Ok(What::Label(l.trim().to_owned()));
        }
        let at = at.unwrap_or("");
        let bad = || {
            Failed(
                EXIT_REFUSED,
                format!("--at {at}: FILE:LINE, such as main.tex:42"),
            )
        };
        let (f, l) = at.rsplit_once(':').ok_or_else(bad)?;
        let l: usize = l.trim().parse().map_err(|_| bad())?;
        if l == 0 || f.is_empty() {
            return Err(bad());
        }
        Ok(What::Line(f.to_owned(), l))
    }

    /// Its name in a file's name: `fig-x`, `main.tex-42`.
    fn slug(&self) -> String {
        let s = match self {
            What::Label(l) => l.clone(),
            What::Line(f, l) => format!("{f}-{l}"),
            What::Change(n) => format!("change-{n}"),
        };
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    }

    fn said(&self) -> String {
        match self {
            What::Label(l) => l.clone(),
            What::Line(f, l) => format!("{f}:{l}"),
            What::Change(n) => format!("change {n}"),
        }
    }
}

/// Where `png` and `where` found it: the build, the page (from 0), the
/// part (points from the page's top left), the PDFs, and what the .aux
/// says of a label (its number and page as printed).
struct Found {
    build: u64,
    page: usize,
    rect: [f64; 4],
    pdf: PathBuf,
    before: Option<(u64, PathBuf)>,
    number: Option<String>,
    printed: Option<String>,
    /// Found from the label's PDF destination, its source not found: the
    /// part a guess around it.
    approximate: bool,
}

/// `\newlabel{L}{{NUMBER}{PAGE}{TITLE}{ANCHOR}{…}}`'s groups.
fn newlabel_fields(line: &str) -> Vec<String> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    // (the second group's contents)
    let Some(i) = line.find("}{").map(|i| i + 2) else {
        return out;
    };
    let mut depth = 0usize;
    let mut cur = String::new();
    for &c in &b[i..] {
        match c {
            b'{' => {
                if depth > 0 {
                    cur.push('{');
                }
                depth += 1;
            }
            b'}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.push('}');
                }
            }
            _ if depth > 0 => cur.push(char::from(c)),
            _ => {}
        }
    }
    out
}

/// The source of a project file (from the root), comments blanked.
fn source(root: &Path, name: &str) -> Option<Vec<u8>> {
    let p = root.join(name.trim_start_matches("./"));
    std::fs::read(&p)
        .or_else(|_| std::fs::read(p.with_extension("tex")))
        .ok()
        .map(|t| crate::shot::uncommented(&t))
}

/// The source's claims on the drawings beside the glyphs found
/// ([`crate::shot::Claims`]).
struct Src<'a> {
    root: &'a Path,
    names: &'a [String],
    codes: &'a [crate::shot::Code],
    /// The element's source (comments blanked), its range, and the bytes
    /// of the words of it found on the page.
    text: &'a [u8],
    range: (usize, usize),
    shown: (usize, usize),
    texts: std::cell::RefCell<std::collections::HashMap<u32, Option<Vec<u8>>>>,
}

impl Src<'_> {
    fn with_text<T>(&self, file: u32, f: impl FnOnce(&[u8]) -> T) -> Option<T> {
        let mut texts = self.texts.borrow_mut();
        let t = texts.entry(file).or_insert_with(|| {
            let name = self.names.get(file as usize)?;
            if crate::shot::rooted(name) {
                return None;
            }
            source(self.root, name)
        });
        t.as_deref().map(f)
    }
}

impl crate::shot::Claims for Src<'_> {
    fn ours(&self, head: bool) -> bool {
        let ((a, b), (first, last)) = (self.range, self.shown);
        let t = self.text;
        let part = if head {
            &t[a.min(t.len())..first.clamp(a, b).min(t.len())]
        } else {
            &t[last.clamp(a, b).min(t.len())..b.min(t.len())]
        };
        crate::shot::has_ink(part)
    }

    fn theirs(&self, code: usize, tail: bool) -> bool {
        let c = self.codes[code];
        self.with_text(c.file, |t| {
            let (s, e) = (c.start as usize, (c.end as usize).max(c.start as usize));
            let Some(env) = crate::shot::envs(t)
                .into_iter()
                .filter(|v| v.start <= s && e <= v.end && v.name != "document")
                .min_by_key(|v| v.end - v.start)
            else {
                return false;
            };
            if tail {
                crate::shot::has_ink(&t[e.min(env.end)..env.end])
            } else {
                crate::shot::has_ink(&t[env.start..s.max(env.start)])
            }
        })
        .unwrap_or(false)
    }
}

/// A page's glyph origins from a shot's answer: its codes.
fn page_codes(p: &Value) -> Vec<crate::shot::Code> {
    let arr = |k: &str| p[k].as_array().cloned().unwrap_or_default();
    let (file, start, end, x, y) = (arr("file"), arr("start"), arr("end"), arr("x"), arr("y"));
    let synth = arr("synth");
    let u = |a: &Value| {
        a.as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(u32::MAX)
    };
    (0..file.len())
        .map(|i| crate::shot::Code {
            file: u(&file[i]),
            start: u(&start[i]),
            end: u(&end[i]),
            synth: synth.get(i).and_then(Value::as_bool).unwrap_or(true),
            x: x[i].as_f64().unwrap_or(f64::NAN),
            y: y[i].as_f64().unwrap_or(f64::NAN),
        })
        .collect()
}

/// The source range of what `png` and `where` look for: a line's bytes;
/// a label's element (its environment, or its paragraph) and the files
/// it inputs, whole, after it; the file each is in, from the root.
fn ranges_of(
    what: &What,
    root: &Path,
    read: &[String],
    main: &str,
) -> Result<Vec<(String, usize, usize)>, Failed> {
    let mut ranges = Vec::new();
    match what {
        What::Change(_) => {
            return Err(Failed(
                EXIT_REFUSED,
                "a change is a diff's: --compare".into(),
            ));
        }
        What::Line(f, l) => {
            let here = std::env::current_dir().unwrap_or_default();
            let p = here.join(f);
            let p = p.canonicalize().unwrap_or(p);
            let top = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            let name = p
                .strip_prefix(&top)
                .map_err(|_| {
                    Failed(
                        EXIT_REFUSED,
                        format!("{f}: not a file of the watch's folder ({})", top.display()),
                    )
                })?
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read(&p).map_err(|e| Failed(EXIT_REFUSED, format!("{f}: {e}")))?;
            let starts: Vec<usize> = std::iter::once(0)
                .chain(
                    text.iter()
                        .enumerate()
                        .filter(|x| *x.1 == b'\n')
                        .map(|x| x.0 + 1),
                )
                .collect();
            let a = *starts
                .get(l - 1)
                .filter(|&&a| a < text.len())
                .ok_or_else(|| {
                    Failed(EXIT_REFUSED, format!("{f} has {} lines", starts.len() - 1))
                })?;
            let b = starts.get(*l).map_or(text.len(), |e| e - 1);
            ranges.push((name, a, b.max(a + 1)));
        }
        What::Label(l) => {
            let mut files: Vec<&String> = read
                .iter()
                .filter(|n| {
                    let e = Path::new(n)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");
                    matches!(e, "tex" | "ltx" | "")
                })
                .collect();
            // (the main file first)
            files.sort_by_key(|n| n.as_str() != main);
            for name in files {
                let Some(t) = source(root, name) else {
                    continue;
                };
                let Some((a, b)) = crate::shot::label_place(&t, l) else {
                    continue;
                };
                let (start, end, _) = crate::shot::labelled(&t, a, b);
                ranges.push((name.clone(), start, end));
                let base = Path::new(name).parent().unwrap_or(Path::new(""));
                for inp in crate::shot::inputs(&t, start, end) {
                    let found = [inp.clone(), format!("{inp}.tex")]
                        .into_iter()
                        .flat_map(|c| [root.join(&c), root.join(base).join(&c)])
                        .find(|p| p.is_file());
                    if let Some(p) = found {
                        let n = p
                            .strip_prefix(root)
                            .map_or_else(|_| inp.clone(), |x| x.to_string_lossy().into_owned());
                        let len = std::fs::metadata(&p)
                            .map_or(0, |m| usize::try_from(m.len()).unwrap_or(usize::MAX));
                        ranges.push((n, 0, len));
                    }
                }
                break;
            }
        }
    }
    Ok(ranges)
}

/// Where an element shows best on a page: its page (from 0), the page's
/// size, drawings and glyph words, and the match ([`crate::shot::find`])
/// of the element's words chosen.
struct Shown {
    page: usize,
    size: (f64, f64),
    marks: Vec<phitex_render::Mark>,
    words: Vec<crate::shot::Word>,
    found: crate::shot::Found,
}

/// `png`'s and `where`'s search: the build of the files as they are now
/// waited for, the label's (or the line's) source range found, the
/// watch's shot of that build taken ([`crate::review::Hub`]'s
/// `phitex/session/shot`), the page that shows the element's words
/// found (those the engine placed its lines on first, then the one its
/// .aux names, then every page) and the part of it the element is drawn
/// on ([`crate::shot::region`]), `context` points around it.
#[allow(clippy::too_many_lines)]
fn locate(
    c: &mut Conn,
    s: &Value,
    what: &What,
    context: Option<&str>,
    timeout: Option<&str>,
) -> Result<Found, Failed> {
    let pad = match context {
        Some(p) => p
            .trim()
            .trim_end_matches("pt")
            .parse::<f64>()
            .ok()
            .filter(|p| p.is_finite() && *p >= 0.0)
            .ok_or_else(|| Failed(EXIT_REFUSED, format!("--context {p}: points, such as 12")))?,
        None => 3.0,
    };
    let t = match timeout {
        Some(t) => duration(t).map_err(|e| Failed(EXIT_REFUSED, e))?,
        None => Some(Duration::from_secs(60)),
    };
    let ms = t.map_or(0, |d| d.as_millis());
    let w = c.call(
        "phitex/session/wait",
        &json!({"timeout_ms": ms}),
        t.map(|d| d + Duration::from_secs(30)),
    )?;
    if w["timed_out"] == true {
        return Err(Failed(
            EXIT_TIMEOUT,
            "timed out: no build has read the files as they are now".into(),
        ));
    }
    let root = PathBuf::from(s["root"].as_str().unwrap_or("."));
    if let Some(said) = crate::review::make_dir(&root).map_err(|e| Failed(EXIT_REFUSED, e))? {
        eprintln!("phitex: {said}");
    }
    let label = match what {
        What::Label(l) => Some(l.clone()),
        What::Line(..) | What::Change(_) => None,
    };
    let shot = |c: &mut Conn, at: &[(String, usize, usize)], pages: &[usize]| {
        c.call(
            "phitex/session/shot",
            &json!({
                "dir": ".phitex/png/builds",
                "at": at.iter().map(|(f, a, b)| json!([f, a, b])).collect::<Vec<_>>(),
                "pages": pages,
                "label": label,
            }),
            Some(Duration::from_secs(600)),
        )
    };
    let v0 = shot(c, &[], &[])?;
    let strs = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect()
    };
    let ranges = ranges_of(
        what,
        &root,
        &strs(&v0["read"]),
        v0["main"].as_str().unwrap_or(""),
    )?;
    let v = if ranges.is_empty() {
        v0
    } else {
        shot(c, &ranges, &[])?
    };
    let build = v["build"].as_u64().unwrap_or(0);
    let pdf = PathBuf::from(v["pdf"].as_str().unwrap_or(""));
    let before = v["before"]["pdf"]
        .as_str()
        .map(|p| (v["before"]["build"].as_u64().unwrap_or(0), PathBuf::from(p)));
    let fields = v["newlabel"]
        .as_str()
        .map(newlabel_fields)
        .unwrap_or_default();
    let number = fields.first().cloned().filter(|x| !x.is_empty());
    let printed = fields.get(1).cloned().filter(|x| !x.is_empty());
    let bytes: std::sync::Arc<[u8]> = std::fs::read(&pdf)
        .map_err(|e| Failed(EXIT_REFUSED, format!("{}: {e}", pdf.display())))?
        .into();
    let mut r = phitex_render::Renderer::new();
    let no_page = |k: usize| {
        Failed(
            EXIT_REFUSED,
            format!("{}: no page {}", pdf.display(), k + 1),
        )
    };
    if ranges.is_empty() {
        // (no \label{…} in the files read: the label's destination, where
        // hyperref made one, else nothing to go by)
        let l = what.said();
        let anchor = fields.get(3).filter(|a| !a.is_empty());
        let place = anchor.and_then(|a| r.dest(&bytes, a));
        let (Some((k, top, _)), true) = (place, v["newlabel"].is_string()) else {
            return Err(Failed(
                EXIT_REFUSED,
                if v["newlabel"].is_string() {
                    format!(
                        "{l}: its \\label is in none of the files the build read (a macro's?), and the PDF has no destination of it (hyperref makes them); the .aux puts it on page {}",
                        printed.as_deref().unwrap_or("?")
                    )
                } else {
                    format!(
                        "{l}: no such label in the build (none of its files has \\label{{{l}}}, its .aux none)"
                    )
                },
            ));
        };
        let (w, h, _) = r.marks(&bytes, k).ok_or_else(|| no_page(k))?;
        let top = top.unwrap_or(0.0);
        let rect = crate::shot::padded([0.0, top, w, (top + h / 3.0).min(h)], pad, (w, h));
        return Ok(Found {
            build,
            page: k,
            rect,
            pdf,
            before,
            number,
            printed,
            approximate: true,
        });
    }
    // (the element's words, as its source has them)
    let (file, a, b) = ranges[0].clone();
    let text = source(&root, &file)
        .ok_or_else(|| Failed(EXIT_REFUSED, format!("{file}: not readable")))?;
    let mut tokens = crate::shot::tokens(&text, a, b);
    for (f, x, y) in &ranges[1..] {
        if let Some(t) = source(&root, f) {
            tokens.extend(crate::shot::tokens(&t, *x, *y));
        }
    }
    if tokens.is_empty() {
        return Err(Failed(
            EXIT_REFUSED,
            format!(
                "{}: its source ({file}:{a}) has no words to find on a page",
                what.said()
            ),
        ));
    }
    // (the pages looked at: the engine's, the .aux's, every page)
    let count = v["count"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0);
    let candidates: Vec<usize> = v["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| k.as_u64().and_then(|k| usize::try_from(k).ok()))
        .collect();
    let aux: Vec<usize> = printed
        .as_deref()
        .and_then(|p| p.parse::<usize>().ok())
        .filter(|&p| p >= 1)
        .map(|p| vec![p - 1, p, p.saturating_sub(2)])
        .unwrap_or_default();
    let mut best: Option<Shown> = None;
    let mut seen = std::collections::HashSet::new();
    for pass in [candidates, aux, (0..count).collect()] {
        for k in pass {
            if k >= count || !seen.insert(k) {
                continue;
            }
            let Some((w, h, marks)) = r.marks(&bytes, k) else {
                continue;
            };
            let words = crate::shot::page_words(&marks);
            for found in crate::shot::find(&tokens, &words) {
                if best.as_ref().is_none_or(|b| found.score > b.found.score) {
                    best = Some(Shown {
                        page: k,
                        size: (w, h),
                        marks: marks.clone(),
                        words: words.clone(),
                        found,
                    });
                }
            }
        }
        // (a page that shows half its words or more: no other looked at)
        if let Some(b) = &best
            && 2 * b.found.pairs.len() >= tokens.len().min(40)
        {
            break;
        }
    }
    let Some(shown) = best else {
        return Err(Failed(
            EXIT_REFUSED,
            format!(
                "{}: none of its words is on any page (its source: {file}:{a})",
                what.said()
            ),
        ));
    };
    let k = shown.page;
    // (the page's origins: where the shot has none of it, asked)
    let mut pv = v["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["page"].as_u64() == u64::try_from(k).ok())
        .cloned();
    let mut names: Vec<String> = strs(&v["files"]);
    if pv.is_none() {
        let v2 = shot(c, &[], &[k])?;
        pv = v2["pages"].as_array().and_then(|p| p.first()).cloned();
        names = strs(&v2["files"]);
    }
    let codes = pv.as_ref().map(page_codes).unwrap_or_default();
    let top = root.canonicalize().unwrap_or_else(|_| root.clone());
    let user = |f: u32| {
        names.get(f as usize).is_some_and(|n| {
            !crate::shot::rooted(n)
                && (top.join(n).is_file() || top.join(format!("{n}.tex")).is_file())
        })
    };
    let in_range = |c: &crate::shot::Code| {
        names.get(c.file as usize).is_some_and(|n| {
            ranges.iter().any(|(f, a, b)| {
                crate::shot::same_file(n, f)
                    && (c.start as usize) < *b
                    && (c.end as usize).max(c.start as usize + 1) > *a
            })
        })
    };
    // (each glyph: the element's, found; another element's, its typed
    // text as the origins place it; or neither)
    let texts: std::cell::RefCell<std::collections::HashMap<u32, Option<Vec<u8>>>> =
        std::cell::RefCell::default();
    let sure = crate::shot::trusted(&codes, &|f, a, b| {
        let mut texts = texts.borrow_mut();
        let t = texts.entry(f).or_insert_with(|| {
            names
                .get(f as usize)
                .filter(|n| !crate::shot::rooted(n))
                .and_then(|n| source(&root, n))
        });
        t.as_deref()
            .is_some_and(|t| crate::shot::whole_words(t, a, b))
    });
    let code_at = crate::shot::codes_at(&codes);
    let mut class: Vec<crate::shot::Class> = shown
        .marks
        .iter()
        .map(|m| match m.kind {
            phitex_render::MarkKind::Glyph { x, y, .. } => match code_at(x, y) {
                Some(c) if sure[c] && user(codes[c].file) && !in_range(&codes[c]) => {
                    crate::shot::Class::Sure(c)
                }
                _ => crate::shot::Class::Neutral,
            },
            _ => crate::shot::Class::Neutral,
        })
        .collect();
    for &(wi, _) in &shown.found.pairs {
        for &m in &shown.words[wi].marks {
            class[m] = crate::shot::Class::Mine;
        }
    }
    let shown_bytes = (
        shown
            .found
            .pairs
            .iter()
            .map(|p| tokens[p.1].start)
            .min()
            .unwrap_or(a),
        shown
            .found
            .pairs
            .iter()
            .map(|p| tokens[p.1].end)
            .max()
            .unwrap_or(b),
    );
    let src = Src {
        root: &root,
        names: &names,
        codes: &codes,
        text: &text,
        range: (a, b),
        shown: shown_bytes,
        texts: std::cell::RefCell::default(),
    };
    let extend: Option<&dyn crate::shot::Claims> = match what {
        What::Label(_) => Some(&src),
        What::Line(..) | What::Change(_) => None,
    };
    let rect = crate::shot::region(&shown.marks, &class, shown.size, extend).ok_or_else(|| {
        Failed(
            EXIT_REFUSED,
            format!(
                "{}: its glyphs are not drawn on page {}",
                what.said(),
                k + 1
            ),
        )
    })?;
    Ok(Found {
        build,
        page: k,
        rect: crate::shot::padded(rect, pad, shown.size),
        pdf,
        before,
        number,
        printed,
        approximate: false,
    })
}

fn found_json(f: &Found, what: &What) -> Value {
    let r = |x: f64| (x * 100.0).round() / 100.0;
    json!({
        "target": what.said(),
        "build": f.build,
        "page": f.page + 1,
        "bbox": f.rect.map(r),
        "pdf": f.pdf.display().to_string(),
        "number": f.number,
        "printed_page": f.printed,
        "approximate": f.approximate,
    })
}

/// `fig:x: page 3, 72.00 96.00 300.00 250.00 pt (Figure 2; build 7)`.
fn where_line(f: &Found, what: &What) -> String {
    let mut about = Vec::new();
    if let Some(n) = &f.number {
        about.push(format!("number {n}"));
    }
    if let Some(p) = f
        .printed
        .as_ref()
        .filter(|p| **p != (f.page + 1).to_string())
    {
        about.push(format!("printed page {p}"));
    }
    about.push(format!("build {}", f.build));
    if f.approximate {
        about.push(
            "approximate: around its PDF destination, its \\label not found in the source".into(),
        );
    }
    let [x0, y0, x1, y1] = f.rect;
    format!(
        "{}: page {}, {x0:.2} {y0:.2} {x1:.2} {y1:.2} pt from the page's top left ({})",
        what.said(),
        f.page + 1,
        about.join("; ")
    )
}

/// `png`'s drawing of what [`locate`] found: written as a PNG at `dpi`
/// to `to` (from here) or .phitex/png/; with `diff`, the build before's
/// beside it.
fn draw(f: &Found, what: &What, dpi: u32, to: Option<&str>, diff: bool) -> Result<Value, Failed> {
    if !(10..=2400).contains(&dpi) {
        return Err(Failed(
            EXIT_REFUSED,
            format!("--dpi {dpi}: from 10 to 2400"),
        ));
    }
    let scale = f64::from(dpi) / 72.0;
    let mut r = phitex_render::Renderer::new();
    let read = |p: &Path| -> Result<std::sync::Arc<[u8]>, Failed> {
        Ok(std::fs::read(p)
            .map_err(|e| Failed(EXIT_REFUSED, format!("{}: {e}", p.display())))?
            .into())
    };
    let now = read(&f.pdf)?;
    let part = r.draw_part(&now, f.page, f.rect, scale).ok_or_else(|| {
        Failed(
            EXIT_REFUSED,
            format!("page {} of {} not drawn", f.page + 1, f.pdf.display()),
        )
    })?;
    // (beside the builds' PDFs: .phitex/png/)
    let dir = f
        .pdf
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from(".phitex/png"), Path::to_path_buf);
    let mut v = found_json(f, what);
    let (bytes, w, h, name) = if diff {
        let Some((m, before)) = &f.before else {
            return Err(Failed(
                EXIT_REFUSED,
                format!(
                    "no build before build {} kept to compare with (the watch keeps the last two builds' PDFs from its start)",
                    f.build
                ),
            ));
        };
        let old = read(before)?;
        let was = r.draw_part(&old, f.page, f.rect, scale).ok_or_else(|| {
            Failed(
                EXIT_REFUSED,
                format!("build {m} has no page {}", f.page + 1),
            )
        })?;
        if (was.w, was.h) != (part.w, part.h) {
            return Err(Failed(
                EXIT_REFUSED,
                format!("build {m}'s page {} is another size", f.page + 1),
            ));
        }
        let (img, w, h, changed) = crate::shot::diff_image(part.w, part.h, &was.rgba, &part.rgba);
        v["before"] = json!(m);
        v["changed"] = json!(changed);
        (
            img,
            w,
            h,
            format!("{}-build{m}-vs-{}.png", what.slug(), f.build),
        )
    } else {
        (
            part.rgba,
            part.w,
            part.h,
            format!("{}-build{}.png", what.slug(), f.build),
        )
    };
    let path = match to {
        Some(t) => std::env::current_dir().unwrap_or_default().join(t),
        None => dir.join(name),
    };
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)
            .map_err(|e| Failed(EXIT_REFUSED, format!("{}: {e}", d.display())))?;
    }
    std::fs::write(&path, crate::shot::png(w, h, &bytes))
        .map_err(|e| Failed(EXIT_REFUSED, format!("{}: {e}", path.display())))?;
    v["png"] = json!(path.display().to_string());
    v["dpi"] = json!(dpi);
    v["width"] = json!(w);
    v["height"] = json!(h);
    Ok(v)
}

/// `session blame`'s answer, a printed line a row: the page, the source
/// lines, the commit (short hash, author, date, subject) or "not
/// committed".
fn print_blame(v: &Value, st: Style) {
    let commits = v["commits"].as_array().cloned().unwrap_or_default();
    for p in v["pages"].as_array().into_iter().flatten() {
        let k = p["page"].as_u64().unwrap_or(0) + 1;
        for l in p["lines"].as_array().into_iter().flatten() {
            let file = l["file"].as_str().unwrap_or("?");
            let (a, z) = (
                l["lines"][0].as_u64().unwrap_or(0),
                l["lines"][1].as_u64().unwrap_or(0),
            );
            let place = if a == z {
                format!("{file}:{a}")
            } else {
                format!("{file}:{a}-{z}")
            };
            let who = if l["uncommitted"].as_bool() == Some(true) {
                "not committed".to_owned()
            } else if let Some(c) = l["commit"]
                .as_u64()
                .and_then(|c| commits.get(usize::try_from(c).ok()?))
            {
                let day = c["time"].as_i64().map(day).unwrap_or_default();
                format!(
                    "{} {} {} {}",
                    c["short"].as_str().unwrap_or(""),
                    c["author"].as_str().unwrap_or(""),
                    day,
                    c["subject"].as_str().unwrap_or("")
                )
            } else {
                "-".to_owned()
            };
            println!("p{k} {:<24} {}", st.dim(&place), who);
        }
    }
}

/// A time (seconds since the epoch) as its UTC date, `2026-11-17`.
fn day(t: i64) -> String {
    // (days since 1970-01-01 to a civil date: Howard Hinnant's algorithm)
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {

    #[test]
    fn newlabels_read() {
        assert_eq!(
            newlabel_fields("\\newlabel{fig:x}{{2}{3}{A {cat}}{figure.caption.2}{}}"),
            ["2", "3", "A {cat}", "figure.caption.2", ""]
        );
        assert_eq!(newlabel_fields("\\newlabel{a}{{1}{1}}"), ["1", "1"]);
    }

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

    #[test]
    fn days_are_utc_dates() {
        assert_eq!(super::day(0), "1970-01-01");
        assert_eq!(super::day(1_763_337_600), "2025-11-17");
        assert_eq!(super::day(951_782_400), "2000-02-29");
    }
}
