//! `phitex watch`: the session kept alive. A change of a project file on
//! disk, or an editor's unsaved buffer (the viewer's `set_file` and
//! `edit`), is given to the same `Build` as a new version of that file,
//! and the build runs again as an edit (to the fixpoint); the outputs are
//! written and the viewer told which pages changed. The files are watched
//! by the system (`notify`: inotify, FSEvents, kqueue). On a terminal,
//! keys (`keys.rs`: `r` rebuilds, `o` opens the viewer, `p` shows the
//! problems again, `h` lists them, `q` quits) and a status line below
//! what is printed; on a pipe, the same as lines (its end quits). Every
//! message the terminal shows is the viewer's too (its console).

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher as _};
use serde_json::{Value, json};

use crate::keys::{self, Key};
use crate::modern::{self, Options, Target, Ui, ms};
use crate::session::{self, Session, Status};
use crate::tex;
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

/// The diagnostics of a build for the viewer.
fn diagnostics(r: &session::Report, log: &str) -> Vec<Value> {
    let mut v: Vec<Value> = r
        .out
        .errors
        .iter()
        .map(|e| json!({"severity": "error", "code": "tex", "message": crate::problems::bare(e)}))
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

/// Standard input's keys (a terminal's, one at a time; escape sequences,
/// the arrows', dropped) or lines, to `tx`; its end quits.
fn read_keys(tx: &mpsc::Sender<Event>, raw: bool) {
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
            // (Ctrl-C: out at once, a build running or not, as the
            // terminal's own Ctrl-C would)
            if b == 0x03 {
                keys::restore();
                eprintln!();
                std::process::exit(130);
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
                && tx.send(Event::Key(k)).is_err()
            {
                return;
            }
        }
    } else {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            if tx.send(Event::Key(line_key(&l))).is_err() {
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
    /// The last output written without errors.
    good: Option<PathBuf>,
    viewers: usize,
    /// The first good build's time (cold): what an edit is measured by.
    cold: Option<Duration>,
    /// The last change's time from its save to the viewer's screen.
    screen: Option<Duration>,
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
        ui.note(&format!(
            "editor `{e}`: not one the viewer can open (a terminal editor?): a double-click on a page shows the \
             place; --editor names one it can (code, emacsclient, nvim, …)"
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
                ui.line(&format!("{} {}", st.dim("viewer:"), st.cyan(v.url())));
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
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("phitex-watch-stdin".into())
            .spawn(move || read_keys(&tx, raw))
            .expect("a thread");
    }
    drop(tx);
    let help = |ui: &Ui| {
        for (k, what) in keys::HELP {
            ui.line(&format!("  {}  {what}", st.bold(&format!("{k:>9}"))));
        }
    };
    if raw {
        ui.line(&st.dim("phitex watch: h lists the keys, q quits"));
    }
    let quit = |ui: &Ui| -> ! {
        ui.status(None, None);
        keys::restore();
        modern::cache_summary(t, ui);
        std::process::exit(0);
    };
    let last = RefCell::new(Last::default());
    // (the status line: the last build and the browsers watching)
    let show = |ui: &Ui, l: &Last| {
        let sep = " · ";
        let mut parts = vec![format!("watching {main_rel}")];
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
        if raw {
            parts.push("h: keys".to_owned());
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
        status.set("writing the outputs");
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
            status.set("drawing the pages for the viewer");
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
        let mut facts = Vec::new();
        if failed {
            let n = r.out.errors.len();
            facts.push(st.red(&format!("{n} error{}", if n == 1 { "" } else { "s" })));
            facts.push(if last.borrow().good.is_some() {
                "last good output kept".to_owned()
            } else {
                "nothing written".to_owned()
            });
        } else {
            let live = r.runs.last().map_or(0, |x| x.live);
            // (an edit: what it ran of what the document has, the rest kept)
            #[allow(clippy::cast_precision_loss)]
            facts.push(if builds > 1 && live > 0 {
                let kept = 100.0 * (1.0 - nodes as f64 / live as f64).max(0.0);
                format!("{nodes} of {live} nodes run ({kept:.0}% kept)")
            } else {
                format!("{nodes} nodes run")
            });
            let n = r.pages();
            let s = if n == 1 { "" } else { "s" };
            facts.push(if builds == 1 {
                format!("{n} page{s}")
            } else if changed.is_empty() {
                format!("no page of {n} changed")
            } else {
                format!(
                    "{} of {n} page{s} changed ({})",
                    changed.len(),
                    view::ranges(changed)
                )
            });
        }
        facts.push(st.bold(&ms(r.wall)));
        let par = r.runs.last().and_then(session::RunInfo::parallel);
        if let Some(p) = &par {
            facts.push(st.dim(p));
        }
        let cold = last.borrow().cold;
        match cold {
            None if !failed => last.borrow_mut().cold = Some(r.wall),
            Some(c) if !failed => {
                if let Some(x) = faster(c, r.wall) {
                    facts.push(st.green(&format!("{x} faster than cold")));
                }
            }
            _ => {}
        }
        // (a save that changed nothing the build reads (vim writes twice)
        // keeps the last change's time to the screen)
        let noop = !failed && builds > 1 && nodes == 0 && changed.is_empty();
        let mark = if failed {
            st.red(st.sym("✗", "failed:"))
        } else {
            st.green(st.sym("✓", "ok:"))
        };
        ui.line(&format!(
            "{mark} build {builds}  {}  {}",
            st.cyan(&place),
            facts.join(st.sym(" · ", ", "))
        ));
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
        let warnings: usize = crate::warnings::summarize(&r.log_text())
            .iter()
            .map(|w| w.1)
            .sum();
        let mut brief = vec![format!(
            "build {builds} {}",
            if failed { "failed" } else { "ok" }
        )];
        if failed {
            let n = r.out.errors.len();
            brief.push(format!("{n} error{}", if n == 1 { "" } else { "s" }));
        } else {
            let n = r.pages();
            brief.push(format!("{n} page{}", if n == 1 { "" } else { "s" }));
        }
        brief.push(ms(r.wall));
        if let Some(p) = &par {
            brief.push(p.clone());
        }
        if warnings > 0 {
            brief.push(format!(
                "{warnings} warning{}",
                if warnings == 1 { "" } else { "s" }
            ));
        }
        {
            let mut l = last.borrow_mut();
            if let (Some(c), false) = (l.cold.filter(|_| builds > 1), failed)
                && let Some(x) = faster(c, r.wall)
            {
                brief.push(format!("{x} faster than cold"));
            }
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
                    Key::Problems => match &last.borrow().problems {
                        Some((errors, log)) => {
                            let n = errors.len();
                            let ws = crate::warnings::summarize(log);
                            if n == 0 && ws.is_empty() {
                                ui.line(&st.green("no errors, no warnings"));
                            }
                            crate::problems::show(ui, errors, log, &t.file);
                            for (k, n, first) in ws {
                                ui.warning(&format!(
                                    "{n} × {k:?}: {}",
                                    first.first().cloned().unwrap_or_default()
                                ));
                            }
                        }
                        None => ui.line("phitex watch: no build yet"),
                    },
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
