//! `phitex watch`: the session kept alive. A change of a project file on
//! disk, or an editor's unsaved buffer (the viewer's `set_file` and
//! `edit`), is given to the same `Build` as a new version of that file,
//! and the build runs again as an edit (to the fixpoint); the outputs are
//! written and the viewer told which pages changed. The files are watched
//! by the system (`notify`: inotify, FSEvents, kqueue). On a terminal,
//! keys (`keys.rs`: `r` rebuilds, `o` opens the viewer, `p` shows the
//! errors again, `w` the warnings, `s` how the build went, `h` lists
//! them, `q` quits; a live tree of the build's phases while it runs) and a status line below
//! what is printed; on a pipe, the same as lines (its end quits). Every
//! message the terminal shows is the viewer's too (its console).

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher as _};
use serde_json::{Value, json};

use crate::keys::{self, Key};
use crate::modern::{self, Options, Target, Ui, ms};
use crate::session::{self, Session, Status};
use crate::tex;
use crate::ui::pad;
use crate::view::{self, Ask, Built, View};

/// What wakes the watch.
enum Event {
    /// Files of the project changed on disk.
    Files(Vec<PathBuf>),
    /// A key (on a pipe: a line) on standard input.
    Key(Key),
    /// The viewer asked for something.
    Asked,
    /// This many browsers show the viewer now.
    Clients(usize),
}

/// The extensions of the files a change of rebuilds.
const SOURCES: &[&str] = &[
    "tex", "sty", "cls", "bib", "bst", "def", "cfg", "clo", "fd", "ltx", "ist", "bbx", "cbx",
    "lbx", "dbx", "png", "jpg", "jpeg", "pdf", "eps", "mp", "txt", "dat", "csv", "tikz", "pgf",
    "inc",
];

/// Whether a change of `p` (under `root`) is one of the project's
/// sources: a source's extension, in no hidden directory.
fn source(root: &Path, p: &Path) -> bool {
    let rel = p.strip_prefix(root).unwrap_or(p);
    !rel.components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        && p.extension()
            .is_some_and(|x| SOURCES.contains(&&*x.to_string_lossy().to_ascii_lowercase()))
}

/// The files the watch wrote itself, with their bytes' hashes: a change
/// to one of them that leaves those bytes is the watch's own.
type Ours = Arc<Mutex<HashMap<PathBuf, u64>>>;

/// The system's file events under `root`, as the project's changed
/// sources (the watch's own writes left out).
fn watcher(
    root: &Path,
    ours: Ours,
    tx: mpsc::Sender<Event>,
) -> notify::Result<notify::RecommendedWatcher> {
    let r = root.to_path_buf();
    let mut w = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
        let Ok(ev) = ev else { return };
        if !matches!(
            ev.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        let mut changed: Vec<PathBuf> = ev.paths.into_iter().filter(|p| source(&r, p)).collect();
        {
            let ours = ours.lock().unwrap_or_else(PoisonError::into_inner);
            changed.retain(|p| {
                let mine = ours
                    .get(p)
                    .or_else(|| p.canonicalize().ok().and_then(|c| ours.get(&c)));
                match mine {
                    Some(&h) => std::fs::read(p).map_or(true, |b| tex::Fnv::new().add(&b).0 != h),
                    None => true,
                }
            });
        }
        if !changed.is_empty() {
            let _ = tx.send(Event::Files(changed));
        }
    })?;
    w.watch(root, RecursiveMode::Recursive)?;
    Ok(w)
}

/// `s` with UTF-16 code units `start..end` replaced by `text` (the
/// viewer's offsets).
fn splice_utf16(s: &str, start: usize, end: usize, text: &str) -> String {
    let at = |off: usize| {
        let mut u = 0;
        for (i, c) in s.char_indices() {
            if u >= off {
                return i;
            }
            u += c.len_utf16();
        }
        s.len()
    };
    let (a, b) = (at(start), at(end.max(start)));
    format!("{}{text}{}", &s[..a], &s[b..])
}

/// A Unicode text as the bytes of its file, read by TeX as characters.
fn tex_text(s: &str) -> String {
    tex::chars(s.as_bytes())
}

/// The diagnostics of a build for the viewer: the engine's, at their
/// places, else the transcript's.
fn diagnostics(r: &session::Report, log: &str) -> Vec<Value> {
    if r.internal.is_none() && !r.diagnostics.is_empty() {
        return r
            .diagnostics
            .iter()
            .map(|d| {
                let mut detail = d.context.clone();
                if let Some(h) = &d.help {
                    detail.push_str("\n\n");
                    detail.push_str(h);
                }
                json!({
                    "severity": if d.severity == phitex_tex::Severity::Error { "error" } else { "warning" },
                    "code": "tex",
                    "message": d.message,
                    "file": d.file.trim_start_matches("./"),
                    "line": d.line,
                    "detail": detail,
                })
            })
            .collect();
    }
    let mut v: Vec<Value> = r
        .out
        .errors
        .iter()
        .map(|e| match &r.internal {
            // (the engine's stop: its sentence, the dump under it)
            Some(d) => json!({"severity": "error", "code": "engine", "message": e, "detail": d}),
            None => {
                json!({"severity": "error", "code": "tex", "message": crate::problems::bare(e)})
            }
        })
        .collect();
    for (k, n, first) in crate::warnings::summarize(&r.log_text()) {
        v.push(json!({
            "severity": "warning",
            "code": format!("{k:?}"),
            "message": format!("{n} × {}", first.first().cloned().unwrap_or_default()),
            "detail": format!("see {log}"),
        }));
    }
    v
}

/// What a line on standard input (a pipe) asks.
fn line_key(l: &str) -> Key {
    let l = l.trim();
    let mut cs = l.chars();
    match (cs.next(), cs.next()) {
        (None, _) => Key::Rebuild,
        (Some(c), None) => Key::of(c).unwrap_or(Key::Other(c)),
        _ if l == "quit" => Key::Quit,
        (Some(c), _) => Key::Other(c),
    }
}

/// The keys answered where they are read, a build running or not: they
/// need nothing of the build.
struct Here {
    ui: Ui,
    /// The viewer's address (none: no viewer, or `--open` starts a program
    /// on the output: the watch's own loop opens that).
    url: Option<String>,
    building: Arc<AtomicBool>,
    /// For the cache's line on the way out.
    target: Target,
}

fn help(ui: &Ui) {
    for (k, what) in keys::HELP {
        ui.line(&format!("  {}  {what}", ui.st.bold(&format!("{k:>9}"))));
    }
}

impl Here {
    /// Key `k` answered here (true), or for the watch's loop.
    fn answer(&self, k: Key) -> bool {
        let ui = &self.ui;
        let busy = self.building.load(Ordering::Relaxed);
        match k {
            Key::Help => help(ui),
            Key::Clear => ui.clear(),
            Key::Other(c) => ui.line(&format!(
                "phitex watch: `{c}` is no key here (h lists them)"
            )),
            Key::Open if self.url.is_some() => {
                let url = self.url.as_deref().unwrap_or_default();
                if !view::open_browser(url) {
                    ui.warning(&format!("could not start a browser: open {url}"));
                }
            }
            // (q or Ctrl-C while a build runs: out now, the build dropped)
            Key::Quit if busy => {
                ui.finish();
                keys::restore();
                ui.line(&ui.st.dim("stopped during a build"));
                modern::cache_summary(&self.target, ui);
                std::process::exit(0);
            }
            Key::Rebuild | Key::Problems | Key::Warnings | Key::Summary if busy => {
                ui.line(
                    &ui.st
                        .dim("phitex watch: a build is running; this comes after it"),
                );
                return false;
            }
            _ => return false,
        }
        true
    }
}

/// Standard input's keys (a terminal's, one at a time; escape sequences,
/// the arrows', dropped) or lines, to `tx` (those not answered `here`);
/// its end quits.
fn read_keys(tx: &mpsc::Sender<Event>, raw: bool, here: &Here) {
    if raw {
        let mut bytes = std::io::stdin().lock().bytes();
        let mut text = Vec::new();
        while let Some(Ok(b)) = bytes.next() {
            if b == 0x1b {
                // (`ESC [ … letter-or-~`, or ESC alone)
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
            // (a character's bytes, whole)
            text.push(b);
            let Ok(t) = std::str::from_utf8(&text) else {
                if text.len() >= 4 {
                    text.clear();
                }
                continue;
            };
            let k = t.chars().next().and_then(Key::of);
            text.clear();
            if let Some(k) = k
                && !here.answer(k)
                && tx.send(Event::Key(k)).is_err()
            {
                return;
            }
        }
    } else {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            let k = line_key(&l);
            if !here.answer(k) && tx.send(Event::Key(k)).is_err() {
                return;
            }
        }
    }
    let _ = tx.send(Event::Key(Key::Quit));
}

/// What the watch knows of its last build, for its keys and status line.
#[derive(Default)]
struct Last {
    /// The status line's facts of the last build, and whether it was good.
    build: Option<(String, bool)>,
    /// Its errors and its log's text, for `p`.
    problems: Option<(Vec<String>, String)>,
    /// Its errors and warnings at their places, rendered, for `p` and `w`.
    placed: (Vec<String>, Vec<String>),
    /// The last output written without errors.
    good: Option<PathBuf>,
    viewers: usize,
    /// The first good build's time (cold): what an edit is measured by.
    cold: Option<Duration>,
    /// The last change's time from its save to the viewer's screen.
    screen: Option<Duration>,
    /// The builds, and the edits' times (those that changed something).
    builds: usize,
    edits: Vec<Duration>,
    /// The last build's summary, for `s`.
    summary: Vec<String>,
}

/// The quit line: `✓ 9 builds · fastest edit 9.2 ms · median 412 ms ·
/// cold 15.38 s`.
fn farewell(st: crate::ui::Style, l: &Last) -> Option<String> {
    if l.builds == 0 {
        return None;
    }
    let sep = st.dim(" · ");
    let mut parts = vec![format!(
        "{} build{}",
        l.builds,
        if l.builds == 1 { "" } else { "s" }
    )];
    let mut e = l.edits.clone();
    e.sort();
    if let Some(f) = e.first() {
        parts.push(format!("fastest edit {}", st.bold(&ms(*f))));
        parts.push(format!("median {}", ms(e[e.len() / 2])));
    }
    if let Some(c) = l.cold {
        parts.push(format!("cold {}", ms(c)));
    }
    Some(format!(
        "{} {}",
        st.green(st.sym("✓", "done:")),
        parts.join(&sep)
    ))
}

/// `rows` as a tree under `head`: `├─ name  value`, the last `└─`.
fn tree(st: crate::ui::Style, head: String, rows: &[(String, String)]) -> Vec<String> {
    let mut out = vec![head];
    for (k, (name, value)) in rows.iter().enumerate() {
        let branch = if k + 1 == rows.len() {
            "└─"
        } else {
            "├─"
        };
        out.push(format!(
            "  {} {} {value}",
            st.dim(branch),
            crate::ui::pad(name, 14, false)
        ));
    }
    out
}

/// `cold / wall` as `14×` or `2.5×`, when an edit beat the cold build.
fn faster(cold: Duration, wall: Duration) -> Option<String> {
    let x = cold.as_secs_f64() / wall.as_secs_f64().max(1e-6);
    (x >= 1.5).then(|| {
        if x < 10.0 {
            format!("{x:.1}×")
        } else {
            format!("{x:.0}×")
        }
    })
}

/// Watch target `t`; never returns.
#[allow(clippy::too_many_lines)]
pub fn main(t: &Target, o: &Options, ui: &Ui) -> ! {
    let st = ui.st;
    let job = modern::job(t, o).unwrap_or_else(|e| {
        ui.error(&e);
        std::process::exit(crate::EXIT_USAGE);
    });
    // (the banner: what is built into what, by which engine)
    if !ui.quiet {
        let threads = job
            .threads
            .or_else(|| std::env::var("PHITEX_THREADS").ok()?.parse().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from));
        ui.line(&format!(
            "{} {}  {} → {}  {}",
            st.bold("PhiTeX"),
            st.dim(env!("CARGO_PKG_VERSION")),
            st.bold(&t.file),
            st.bold(&format!("{}.{}", job.jobname, job.engine.output.ext())),
            st.dim(&format!("{} · {threads} threads", job.engine.name))
        ));
    }
    for n in session::waiting_notes(&job) {
        ui.line(&format!("{}: {n}", st.yellow("note")));
    }
    let status = Arc::new(Status::default());
    let what = modern::what(t, &job);
    let spin = ui.spin(what.clone(), status.clone());
    let mut s = Session::new(job, status.clone(), &mut |n: &str| ui.note(n)).unwrap_or_else(|e| {
        ui.error(&e);
        std::process::exit(crate::EXIT_USAGE);
    });
    spin.done();
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let main_file = root.join(&s.job.path);
    let main_rel = s
        .job
        .path
        .to_string_lossy()
        .trim_start_matches("./")
        .to_owned();
    let (tx, rx) = mpsc::channel::<Event>();
    // (a terminal editor, or an unknown one: the viewer shows the place
    // instead of opening it)
    let editor = crate::editor::Editor::configured(t.editor.as_deref()).unwrap_or_else(|e| {
        ui.line(&format!(
            "{}: editor `{e}`: the viewer can't open it (--editor code, nvim, emacsclient, …)",
            st.yellow("note")
        ));
        None
    });
    let view = if o.view {
        match View::start(editor, o.port) {
            Ok(v) => {
                ui.tap(v.console());
                let wake = Mutex::new(tx.clone());
                v.on_ask(Box::new(move || {
                    let _ = wake
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send(Event::Asked);
                }));
                let wake = Mutex::new(tx.clone());
                v.on_clients(Box::new(move |n| {
                    let _ = wake
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send(Event::Clients(n));
                }));
                ui.line(&format!(
                    "{}  {}  {}",
                    st.dim("viewer"),
                    st.cyan(v.url()),
                    st.dim("(o opens it)")
                ));
                ui.event(&json!({"event": "viewer", "url": v.url()}));
                if o.open && t.viewer.is_none() && !view::open_browser(v.url()) {
                    ui.warning("could not start a browser");
                }
                Some(v)
            }
            Err(e) => {
                ui.warning(&format!("the viewer: {e}"));
                None
            }
        }
    } else {
        None
    };
    let ours: Ours = Arc::default();
    // (kept for the watch's life: dropping it stops the events)
    let _files = watcher(&root, ours.clone(), tx.clone()).unwrap_or_else(|e| {
        ui.error(&format!("can't watch {}: {e}", root.display()));
        std::process::exit(crate::EXIT_USAGE);
    });
    let raw = !ui.json && keys::raw();
    let building = Arc::new(AtomicBool::new(false));
    {
        let tx = tx.clone();
        let here = Here {
            ui: ui.clone(),
            url: view
                .as_ref()
                .filter(|_| t.viewer.is_none())
                .map(|v| v.url().to_owned()),
            building: building.clone(),
            target: t.clone(),
        };
        std::thread::Builder::new()
            .name("phitex-watch-stdin".into())
            .spawn(move || read_keys(&tx, raw, &here))
            .expect("a thread");
    }
    drop(tx);
    if raw {
        ui.keys(keys::BAR);
    }
    let last = RefCell::new(Last::default());
    let quit = |ui: &Ui| -> ! {
        ui.finish();
        keys::restore();
        if let Some(l) = farewell(st, &last.borrow()) {
            ui.line(&l);
        }
        modern::cache_summary(t, ui);
        std::process::exit(0);
    };
    // (the status line: the last build and the browsers watching)
    let show = |ui: &Ui, l: &Last| {
        let sep = " · ";
        let mut parts = vec![main_rel.clone()];
        if let Some((b, _)) = &l.build {
            parts.push(b.clone());
            if let Some(d) = l.screen {
                parts.push(format!("⚡ {} to the screen", ms(d)));
            }
        }
        if view.is_some() {
            parts.push(match l.viewers {
                0 => "no viewer open".to_owned(),
                1 => "1 viewer".to_owned(),
                n => format!("{n} viewers"),
            });
        }
        ui.status(Some(&parts.join(sep)), l.build.as_ref().map(|b| b.1));
    };
    // (the editor's buffers, as Unicode text, by file name)
    let mut buffers: HashMap<String, String> = HashMap::new();
    let mut builds = 0usize;
    // (the last output written without errors, and its PDF for the viewer)
    let mut last_pdf = None;
    let mut last_hashes: Vec<u64> = Vec::new();
    // (`viewer = "zathura"`: --open starts it on the output, once)
    let open_cmd = t.viewer.clone().filter(|_| o.open);
    let mut started = false;
    // (`since`: when the change came; none for the cold build)
    let mut rebuild = |s: &mut Session, why: &str, since: Option<Instant>| {
        builds += 1;
        last.borrow_mut().builds = builds;
        status.take();
        building.store(true, Ordering::Relaxed);
        if let Some(v) = &view {
            v.building(true);
        }
        let spin = ui.spin(format!("{what}: build {builds} ({why})"), status.clone());
        // (a build that fails inside the engine — a panic: the pieces did
        // not settle — is reported, and the watch goes on waiting)
        let r = match crate::ui::catch(|| s.build()) {
            Ok(r) => r,
            Err(m) => {
                spin.done();
                building.store(false, Ordering::Relaxed);
                ui.failed(&format!("build {builds} ({why})"), &m);
                let first = m.lines().next().unwrap_or("the engine panicked");
                ui.event(
                    &json!({"event": "failed", "build": builds, "why": why, "message": first}),
                );
                if let Some(v) = &view {
                    v.built(&Built {
                        pdf: last_pdf.clone(),
                        hashes: last_hashes.clone(),
                        history: 2,
                        ms: 0.0,
                        since_ms: None,
                        diagnostics: vec![
                            json!({"severity": "error", "code": "engine", "message": first}),
                        ],
                    });
                    v.building(false);
                }
                let mut l = last.borrow_mut();
                l.build = Some((format!("build {builds} failed in the engine"), false));
                show(ui, &l);
                return;
            }
        };
        let failed = !r.out.errors.is_empty();
        status.set("writing files");
        // (the watcher waits on the lock while the files are written, then
        // knows them as the watch's own; a build with errors leaves the last
        // good output in place)
        let mut mine = ours.lock().unwrap_or_else(PoisonError::into_inner);
        let written = match s.write(&r, !failed) {
            Ok(w) => {
                let ours = &mut *mine;
                for (p, h) in &w.files {
                    let p = p.canonicalize().unwrap_or_else(|_| root.join(p));
                    ours.insert(p, *h);
                }
                w.main
            }
            Err(e) => {
                ui.error(&e);
                None
            }
        };
        drop(mine);
        status.set("");
        if !failed && let Some(out) = &written {
            last.borrow_mut().good = Some(out.clone());
        }
        if let Some(v) = &view {
            if !failed {
                last_pdf = match r.main_output() {
                    Some(("pdf", b)) => Some(session::shared(b)),
                    Some((_, dvi)) => match crate::dvipdf::dvi_to_pdf(
                        dvi,
                        &format!("{}.pdf", s.job.jobname),
                        s.job.pdf_date,
                    ) {
                        Ok(p) => Some(session::shared(&p)),
                        Err(e) => {
                            ui.warning(&format!("the viewer's PDF of the DVI file: {e}"));
                            None
                        }
                    },
                    None => None,
                };
            }
            let log = s
                .job
                .out_dir
                .join(format!("{}.log", s.job.jobname))
                .display()
                .to_string();
            status.set("viewer");
            if !failed {
                last_hashes.clone_from(&r.out.page_hashes);
            }
            v.built(&Built {
                pdf: last_pdf.clone(),
                hashes: last_hashes.clone(),
                history: r.history(),
                ms: r.wall.as_secs_f64() * 1e3,
                since_ms: since.map(|t| t.elapsed().as_secs_f64() * 1e3),
                diagnostics: diagnostics(&r, &log),
            });
            v.building(false);
            status.set("");
        }
        spin.done();
        building.store(false, Ordering::Relaxed);
        // (the place edited: the main file's first changed line)
        let place = match r.edit_line {
            Some(l) => why
                .split(", ")
                .map(|w| {
                    if w == main_rel || w == format!("{main_rel} (editor)") {
                        format!("{main_rel}:{l}")
                    } else {
                        w.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
            None => why.to_owned(),
        };
        let nodes = r.runs.last().map_or(0, |x| x.commands_run);
        let changed = &r.out.pages_changed;
        // (a save that changed nothing the build reads (vim writes twice)
        // keeps the last change's time to the screen)
        let noop = !failed && builds > 1 && nodes == 0 && changed.is_empty();
        let placed = (
            crate::problems::rendered(s, &r, false, st.color, ui.verbose > 0),
            crate::problems::rendered(s, &r, true, st.color, ui.verbose > 0),
        );
        let warnings: usize = if r.diagnostics.is_empty() {
            crate::warnings::summarize(&r.log_text())
                .iter()
                .map(|w| w.1)
                .sum()
        } else {
            placed.1.len()
        };
        last.borrow_mut().placed = placed;
        let live = r.runs.last().map_or(0, |x| x.live);
        // (aligned columns: what changed, the work, the time; then the
        // speed against the cold build, parallelism when there was some,
        // the warnings)
        let mut facts = Vec::new();
        if failed {
            let n = r.out.errors.len();
            facts.push(st.red(&format!("{n} error{}", if n == 1 { "" } else { "s" })));
            if last.borrow().good.is_some() {
                facts.push("last good PDF kept".to_owned());
            }
        } else if noop {
            facts.push("unchanged".to_owned());
        } else {
            let n = r.pages();
            facts.push(if builds == 1 {
                format!("{n} page{}", if n == 1 { "" } else { "s" })
            } else if changed.is_empty() {
                "no page changed".to_owned()
            } else {
                format!(
                    "page{} {}",
                    if changed.len() == 1 { "" } else { "s" },
                    view::ranges(changed)
                )
            });
            facts.push(if builds > 1 && live > 0 {
                format!("{nodes}/{live} nodes")
            } else {
                format!("{nodes} nodes")
            });
        }
        let mut tail = Vec::new();
        let cold = last.borrow().cold;
        match cold {
            None if !failed => last.borrow_mut().cold = Some(r.wall),
            Some(c) if !failed && !noop => {
                last.borrow_mut().edits.push(r.wall);
                if let Some(x) = faster(c, r.wall) {
                    tail.push(st.green(&format!("{x} faster")));
                }
            }
            _ => {}
        }
        if let Some(p) = r
            .runs
            .last()
            .and_then(session::RunInfo::speedup)
            .filter(|&p| p >= 1.2)
        {
            tail.push(st.cyan(&format!("{p:.1}× parallel")));
        }
        if warnings > 0 {
            tail.push(st.yellow(&format!(
                "{warnings} warning{} (w)",
                if warnings == 1 { "" } else { "s" }
            )));
        }
        let mark = if failed {
            st.red(st.sym("✗", "failed:"))
        } else {
            st.green(st.sym("✓", "ok:"))
        };
        let sep = st.dim(st.sym(" · ", ", "));
        let mut out = format!(
            "{mark} {}  {}  {}  {}",
            pad(&builds.to_string(), 2, true),
            pad(&st.cyan(&place), 10, false),
            pad(&facts.join(&sep), 26, false),
            pad(&st.bold(&ms(r.wall)), 9, true)
        );
        if !tail.is_empty() {
            out.push_str("  ");
            out.push_str(&tail.join("  "));
        }
        ui.line(out.trim_end());
        // (for `s`: the build's phases, its reuse, its parallelism)
        {
            let mut rows = Vec::new();
            if builds > 1 && live > 0 {
                let kept = live.saturating_sub(usize::try_from(nodes).unwrap_or(live));
                rows.push((
                    "reuse".to_owned(),
                    format!("{kept}/{live} nodes kept ({}%)", kept * 100 / live.max(1)),
                ));
            } else {
                rows.push(("nodes".to_owned(), format!("{nodes} run")));
            }
            for (name, d) in status.take() {
                rows.push((name, ms(d)));
            }
            if let Some(x) = r.runs.last() {
                // (a run too short to measure says only its threads)
                let mut p = match x.speedup().filter(|_| x.engine_wall.as_millis() >= 50) {
                    Some(p) => format!("{p:.1}× on {} threads", x.threads),
                    None => format!(
                        "{} thread{}",
                        x.threads,
                        if x.threads == 1 { "" } else { "s" }
                    ),
                };
                // (the chain is the whole document's: an edit that ran fewer
                // nodes says nothing by it)
                if let Some(c) = x
                    .critical_path
                    .filter(|&c| c > 0 && x.commands_run >= c as u64)
                {
                    p.push_str(&format!(" · longest chain {c} nodes"));
                }
                if x.peak_running > 0 {
                    p.push_str(&format!(" · at most {} at once", x.peak_running));
                }
                rows.push(("parallel".to_owned(), p));
            }
            let head = format!(
                "{}  {}  {}",
                st.bold(&format!("build {builds}")),
                st.cyan(&place),
                st.bold(&ms(r.wall))
            );
            last.borrow_mut().summary = tree(st, head, &rows);
        }
        ui.event(&json!({
            "event": "build",
            "build": builds,
            "why": why,
            "edit": r.edit_line.map(|l| format!("{main_rel}:{l}")),
            "ok": !failed,
            "output": written.as_ref().map(|p| p.display().to_string()),
            "pages": r.pages(),
            "pages_changed": changed,
            "nodes_run": nodes,
            "parallel": r.runs.last().map(|x| json!({
                "threads": x.threads,
                "busy_ms": x.busy.as_secs_f64() * 1e3,
                "engine_ms": x.engine_wall.as_secs_f64() * 1e3,
                "critical_path": x.critical_path,
                "peak_running": x.peak_running,
            })),
            "wall_ms": r.wall.as_secs_f64() * 1e3,
            "errors": r.out.errors,
        }));
        modern::report(
            t,
            s,
            &r,
            written.as_deref(),
            &Options {
                quiet: true,
                ..Options::default()
            },
            ui,
            false,
        );
        // (the status line: the build's facts, without the colours)
        let mut brief = vec![format!("build {builds}")];
        if failed {
            let n = r.out.errors.len();
            brief.push(format!("{n} error{}", if n == 1 { "" } else { "s" }));
        } else {
            let n = r.pages();
            brief.push(format!("{n} page{}", if n == 1 { "" } else { "s" }));
        }
        brief.push(ms(r.wall));
        if warnings > 0 {
            brief.push(format!(
                "{warnings} warning{}",
                if warnings == 1 { "" } else { "s" }
            ));
        }
        {
            let mut l = last.borrow_mut();
            if !noop {
                l.screen = None;
            }
            l.build = Some((brief.join(" · "), !failed));
            l.problems = Some((r.out.errors.clone(), r.log_text()));
            show(ui, &l);
        }
        // (--open with a viewer program: started on the first output)
        let good = last.borrow().good.clone();
        if let (Some(cmd), Some(out), false) = (&open_cmd, &good, started) {
            started = true;
            if let Err(e) = view::open_with(cmd, out) {
                ui.warning(&format!("can't start {cmd}: {e}"));
            }
        }
    };
    rebuild(&mut s, "cold", None);
    let set = |s: &mut Session, buffers: &HashMap<String, String>, file: &str| {
        let text = buffers.get(file).map(|b| tex_text(b));
        if file == main_rel {
            let t = text.unwrap_or_else(|| {
                std::fs::read(&main_file)
                    .map(|b| tex::chars(&b))
                    .unwrap_or_default()
            });
            s.set_text(t);
        } else {
            s.set_buffer(file, text.as_deref());
        }
    };
    while let Ok(first) = rx.recv() {
        let came = Instant::now();
        // (what came meanwhile is one edit)
        let mut events = vec![first];
        while let Ok(e) = rx.recv_timeout(Duration::from_millis(2)) {
            events.push(e);
        }
        let mut why = Vec::new();
        let mut again = false;
        for e in events {
            match e {
                Event::Key(k) => match k {
                    Key::Quit => quit(ui),
                    Key::Rebuild => {
                        again = true;
                        why.push("asked".to_owned());
                    }
                    Key::Open => {
                        let good = last.borrow().good.clone();
                        match (&view, &t.viewer, good) {
                            (_, Some(cmd), Some(out)) => {
                                if let Err(e) = view::open_with(cmd, &out) {
                                    ui.warning(&format!("can't start {cmd}: {e}"));
                                }
                            }
                            (Some(v), _, _) => {
                                if !view::open_browser(v.url()) {
                                    ui.warning(&format!(
                                        "could not start a browser: open {}",
                                        v.url()
                                    ));
                                }
                            }
                            _ => ui.line("phitex watch: no viewer (--no-view) and no output yet"),
                        }
                    }
                    Key::Problems if !last.borrow().placed.0.is_empty() => {
                        for p in &last.borrow().placed.0 {
                            ui.line(p);
                        }
                    }
                    Key::Warnings if !last.borrow().placed.1.is_empty() => {
                        let l = last.borrow();
                        let max = if ui.verbose > 0 { usize::MAX } else { 20 };
                        for p in l.placed.1.iter().take(max) {
                            ui.line(p);
                        }
                        if l.placed.1.len() > max {
                            ui.line(&st.dim(&format!(
                                "… {} warnings in all (-v shows every one)",
                                l.placed.1.len()
                            )));
                        }
                    }
                    Key::Problems => match &last.borrow().problems {
                        Some((errors, _)) if errors.is_empty() => {
                            ui.line(&st.green("no errors"));
                        }
                        Some((errors, log)) => crate::problems::show(ui, errors, log, &t.file),
                        None => ui.line("phitex watch: no build yet"),
                    },
                    Key::Warnings => match &last.borrow().problems {
                        Some((_, log)) => {
                            let ws = crate::warnings::summarize(log);
                            if ws.is_empty() {
                                ui.line(&st.green("no warnings"));
                            }
                            for (k, n, first) in ws {
                                ui.warning(&format!("{n} × {k:?}"));
                                for l in first {
                                    ui.line(&st.dim(&format!("  {l}")));
                                }
                            }
                        }
                        None => ui.line("phitex watch: no build yet"),
                    },
                    Key::Summary => {
                        let l = last.borrow();
                        if l.summary.is_empty() {
                            ui.line("phitex watch: no build yet");
                        }
                        for line in &l.summary {
                            ui.line(line);
                        }
                    }
                    Key::Clear => ui.clear(),
                    Key::Help => help(ui),
                    Key::Other(c) => ui.line(&format!(
                        "phitex watch: `{c}` is no key here (h lists them)"
                    )),
                },
                Event::Clients(n) => {
                    let mut l = last.borrow_mut();
                    if n > l.viewers {
                        ui.line(&st.dim(&format!("viewer: a browser joined ({n} open)")));
                    } else {
                        ui.line(&st.dim(&format!("viewer: a browser left ({n} open)")));
                    }
                    l.viewers = n;
                    show(ui, &l);
                }
                Event::Files(ps) => {
                    for p in ps {
                        let rel = p
                            .strip_prefix(&root)
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .into_owned();
                        // (a buffer the editor saved is the file again)
                        if let Some(b) = buffers.get(&rel)
                            && std::fs::read(&p).is_ok_and(|d| d == b.as_bytes())
                        {
                            buffers.remove(&rel);
                            set(&mut s, &buffers, &rel);
                        }
                        if rel == main_rel && !buffers.contains_key(&rel) {
                            set(&mut s, &buffers, &rel);
                        }
                        why.push(rel);
                        again = true;
                    }
                }
                Event::Asked => {
                    for a in view.as_ref().map(View::take_asks).unwrap_or_default() {
                        let file = match a {
                            Ask::Painted(us) => {
                                let mut l = last.borrow_mut();
                                l.screen = Some(Duration::from_micros(us));
                                show(ui, &l);
                                continue;
                            }
                            Ask::Rebuild => {
                                why.push("asked in the viewer".to_owned());
                                again = true;
                                continue;
                            }
                            Ask::SetFile { file, text } => {
                                buffers.insert(file.clone(), text);
                                file
                            }
                            Ask::Edit {
                                file,
                                start,
                                end,
                                text,
                            } => {
                                let cur = buffers.get(&file).cloned().unwrap_or_else(|| {
                                    std::fs::read(root.join(&file))
                                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                                        .unwrap_or_default()
                                });
                                buffers.insert(file.clone(), splice_utf16(&cur, start, end, &text));
                                file
                            }
                        };
                        if file.is_empty() || file.starts_with('/') || file.contains("..") {
                            buffers.remove(&file);
                            continue;
                        }
                        set(&mut s, &buffers, &file);
                        why.push(format!("{file} (editor)"));
                        again = true;
                    }
                }
            }
        }
        if again {
            why.dedup();
            rebuild(&mut s, &why.join(", "), Some(came));
        }
    }
    quit(ui)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_as_keys() {
        assert_eq!(line_key(""), Key::Rebuild);
        assert_eq!(line_key(" q "), Key::Quit);
        assert_eq!(line_key("quit"), Key::Quit);
        assert_eq!(line_key("xyz"), Key::Other('x'));
    }

    #[test]
    fn utf16_splices() {
        assert_eq!(splice_utf16("héllo", 1, 2, "e"), "hello");
        assert_eq!(splice_utf16("a😀b", 3, 3, "c"), "a😀cb");
        assert_eq!(splice_utf16("ab", 5, 9, "c"), "abc");
    }
}
