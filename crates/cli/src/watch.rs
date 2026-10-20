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
use std::collections::{HashMap, VecDeque};
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
    /// File `0`'s buffer changed (applied already, while a build ran).
    Edited(String),
    /// The build running on its own thread ended: the watch joins it at
    /// once (no poll).
    Built,
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
    tx: mpsc::Sender<(Instant, Event)>,
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
                    Some(&h) => tex::fingerprint_file(p) != Some(h),
                    None => true,
                }
            });
        }
        if !changed.is_empty() {
            let _ = tx.send((Instant::now(), Event::Files(changed)));
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
/// places, each a whole problem (the viewer's `Problem`: its line, the
/// culprit underlined, TeX's context and help), else the transcript's.
fn diagnostics(s: &Session, r: &session::Report, log: &str) -> Vec<Value> {
    if r.internal.is_none() && !r.diagnostics.is_empty() {
        return r
            .diagnostics
            .iter()
            .map(|d| {
                let p = crate::problems::from_diagnostic(d, s.source(&d.file).as_deref(), false);
                let chars = |b: usize| p.text.get(..b.min(p.text.len())).map_or(0, |t| t.chars().count());
                let error = d.severity == phitex_tex::Severity::Error;
                let engine = d.severity == phitex_tex::Severity::Diagnostic;
                let excerpt = (!p.text.is_empty()).then(|| {
                    json!({"text": p.text, "start": chars(p.start), "len": chars(p.end).saturating_sub(chars(p.start))})
                });
                json!({
                    "severity": if error { "error" } else if engine { "diagnostic" } else { "warning" },
                    "code": "tex",
                    "message": d.message,
                    "notes": p.label.iter().collect::<Vec<_>>(),
                    "help": d.help.iter().flat_map(|h| h.lines()).collect::<Vec<_>>(),
                    "suggestions": [],
                    "file": p.file,
                    "line": d.line,
                    "col": excerpt.as_ref().map(|_| chars(p.start) + 1),
                    "excerpt": excerpt,
                    "context": [],
                    "included": [],
                    "box": null,
                    "detail": d.context,
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

/// The diff's state told the viewer, its files known as the watch's own,
/// a line on the terminal (none: no diff).
fn diff_told(ui: &Ui, v: &View, ours: &Ours, c: Option<&crate::compare::Compare>) {
    let Some(c) = c else { return };
    {
        let mut mine = ours.lock().unwrap_or_else(PoisonError::into_inner);
        for (p, h) in &c.wrote {
            mine.insert(p.canonicalize().unwrap_or_else(|_| p.clone()), *h);
        }
    }
    let state = c.state(false);
    let n = state["changes"].as_array().map_or(0, Vec::len);
    let errors = state["errors"].as_u64().unwrap_or(0);
    v.diff(state, c.pdf.clone(), c.hashes.clone(), c.tex.clone());
    let st = ui.st;
    let mut line = format!(
        "diff {}: {n} change{}",
        c.label,
        if n == 1 { "" } else { "s" }
    );
    if errors > 0 {
        line.push_str(&st.red(&format!(
            " · {errors} error{} in the marked-up document",
            if errors == 1 { "" } else { "s" }
        )));
    }
    ui.line(&line);
    // (its first errors, as a build's; p shows them all while the diff is shown)
    for e in c.errors(st.color).iter().take(3) {
        ui.line(e);
    }
    ui.event(&json!({"event": "diff", "label": c.label, "changes": n, "errors": errors}));
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
    /// The wait for the build said already (this build).
    told: Arc<AtomicBool>,
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
                // (said once a build: a held key says nothing more)
                if !self.told.swap(true, Ordering::Relaxed) {
                    ui.line(
                        &ui.st
                            .dim("phitex watch: a build is running; this comes after it"),
                    );
                }
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
fn read_keys(tx: &mpsc::Sender<(Instant, Event)>, raw: bool, here: &Here) {
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
                && tx.send((Instant::now(), Event::Key(k))).is_err()
            {
                return;
            }
        }
    } else {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            let k = line_key(&l);
            if !here.answer(k) && tx.send((Instant::now(), Event::Key(k))).is_err() {
                return;
            }
        }
    }
    let _ = tx.send((Instant::now(), Event::Key(Key::Quit)));
}

/// What the watch knows of its last build, for its keys and status line.
#[derive(Default)]
struct Last {
    /// The status line's facts of the last build, and whether it was good.
    build: Option<(String, bool)>,
    /// Its errors and its log's text, for `p`.
    problems: Option<(Vec<String>, String)>,
    /// Its diagnostics, rendered at their places for `p` and `w` when
    /// asked (not at every build: a long document's are many), and how
    /// many errors and warnings among them are shown so.
    diags: Vec<phitex_tex::Diagnostic>,
    placed: (usize, usize),
    /// The last output written (errors or not, as pdfTeX writes it).
    good: Option<PathBuf>,
    viewers: usize,
    /// The first good build's time (cold): what an edit is measured by.
    cold: Option<Duration>,
    /// The last change's time from its save to the viewer's screen.
    screen: Option<Duration>,
    /// Every change's time from its save to the screen, for the quit
    /// line.
    screens: Vec<Duration>,
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
    let mut sc = l.screens.clone();
    sc.sort();
    if let Some(f) = sc.first() {
        parts.push(format!(
            "to the screen {} (median {})",
            st.bold(&ms(*f)),
            ms(sc[sc.len() / 2])
        ));
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
        std::process::exit(crate::EXIT_FAILED);
    });
    // (the banner: what is built into what, by which engine)
    if !ui.quiet {
        let threads = session::threads(job.threads);
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
        std::process::exit(crate::EXIT_FAILED);
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
    let (tx, rx) = mpsc::channel::<(Instant, Event)>();
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
                // (the diff's picker: the project's git history)
                let here = root.clone();
                v.on_log(Box::new(move |skip, limit| {
                    crate::compare::Versions::log(&crate::compare::Git::open(&here)?, skip, limit)
                }));
                let wake = Mutex::new(tx.clone());
                v.on_ask(Box::new(move || {
                    let _ = wake
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send((Instant::now(), Event::Asked));
                }));
                let wake = Mutex::new(tx.clone());
                v.on_clients(Box::new(move |n| {
                    let _ = wake
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send((Instant::now(), Event::Clients(n)));
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
        std::process::exit(crate::EXIT_FAILED);
    });
    let raw = !ui.json && keys::raw();
    let building = Arc::new(AtomicBool::new(false));
    let told = Arc::new(AtomicBool::new(false));
    {
        let tx = tx.clone();
        let here = Here {
            ui: ui.clone(),
            url: view
                .as_ref()
                .filter(|_| t.viewer.is_none())
                .map(|v| v.url().to_owned()),
            building: building.clone(),
            told: told.clone(),
            target: t.clone(),
        };
        std::thread::Builder::new()
            .name("phitex-watch-stdin".into())
            .spawn(move || read_keys(&tx, raw, &here))
            .expect("a thread");
    }
    // (the build's thread's end, sent to the loop that waits on it)
    let built = Mutex::new(tx.clone());
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
    // (`meanwhile`: called again and again while the build runs, on this
    // thread: the build runs on one of its own and sends `Event::Built`
    // as it ends, which `meanwhile` answers with true)
    let mut rebuild = |s: &mut Session,
                       why: &str,
                       since: Option<Instant>,
                       meanwhile: &mut dyn FnMut() -> bool| {
        builds += 1;
        if let Some(t) = since
            && !phitex_tex::timeline::active()
        {
            phitex_tex::timeline::begin_at(t);
        }
        phitex_tex::timeline::lap("watch: the build begins");
        if let (Some(t), true) = (since, phitex_tex::tex_time()) {
            eprintln!(
                "watch phases: build {builds} starts {:?} after its change",
                t.elapsed()
            );
        }
        last.borrow_mut().builds = builds;
        status.take();
        building.store(true, Ordering::Relaxed);
        told.store(false, Ordering::Relaxed);
        if let Some(v) = &view {
            v.building(true);
        }
        let spin = ui.spin(format!("{what}: build {builds} ({why})"), status.clone());
        // (a build that fails inside the engine — a panic: the pieces did
        // not settle — is reported, and the watch goes on waiting)
        let ran = std::thread::scope(|sc| {
            let h = sc.spawn(|| {
                let r = crate::ui::catch(|| s.build());
                let _ = built
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send((Instant::now(), Event::Built));
                r
            });
            while !meanwhile() {}
            h.join()
                .unwrap_or_else(|_| Err("the build's thread panicked".to_owned()))
        });
        // (an edit the build took while it ran: timed from it)
        let since = match (since, s.absorbed_at()) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let mut r = match ran {
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
        phitex_tex::timeline::lap("watch: the build's thread joined");
        let failed = !r.out.errors.is_empty();
        // (`PHITEX_TEX_TIME`: the watch's own time after the build)
        let tw = std::time::Instant::now();
        status.set("writing files");
        // (the watcher waits on the lock while the files are written, then
        // knows them as the watch's own)
        let mut mine = ours.lock().unwrap_or_else(PoisonError::into_inner);
        // (every build writes the output pdfTeX would for its input, errors
        // or not: the pages shipped; none when nothing was shipped)
        let written = match s.write(&r, true) {
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
        phitex_tex::timeline::lap("watch: files written (the PDF, the log, the aux files)");
        if phitex_tex::tex_time() {
            eprintln!(
                "watch phases: build {:?}, files written {:?}",
                r.wall,
                tw.elapsed()
            );
        }
        status.set("");
        if let Some(out) = &written {
            last.borrow_mut().good = Some(out.clone());
        }
        if let Some(v) = &view {
            {
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
            last_hashes.clone_from(&r.out.page_hashes);
            v.built(&Built {
                pdf: last_pdf.clone(),
                hashes: last_hashes.clone(),
                history: r.history(),
                ms: r.wall.as_secs_f64() * 1e3,
                since_ms: since.map(|t| t.elapsed().as_secs_f64() * 1e3),
                diagnostics: diagnostics(s, &r, &log),
            });
            v.building(false);
            status.set("");
            phitex_tex::timeline::lap("watch: the viewer's PDF and its diagnostics");
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
        let tpl = std::time::Instant::now();
        let placed = (
            crate::problems::shown_count(&r.diagnostics, false),
            crate::problems::shown_count(&r.diagnostics, true),
        );
        phitex_tex::timeline::lap("watch: problems placed");
        if phitex_tex::tex_time() {
            eprintln!("watch phases: problems placed {:?}", tpl.elapsed());
        }
        let warnings: usize = if r.diagnostics.is_empty() {
            crate::warnings::summarize(&r.log_text())
                .iter()
                .map(|w| w.1)
                .sum()
        } else {
            placed.1
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
            let n = r.pages();
            facts.push(if n == 0 {
                "no PDF".to_owned()
            } else {
                format!("{n} page{}", if n == 1 { "" } else { "s" })
            });
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
                if x.batches > 0 {
                    rows.push((
                        "batches".to_owned(),
                        format!(
                            "{} · {} of one job · largest {}",
                            x.batches, x.batches_inline, x.max_batch
                        ),
                    ));
                }
                let waits: Vec<String> = x
                    .waits_by_kind
                    .iter()
                    .filter(|(_, n)| *n > 0)
                    .map(|(k, n)| format!("{n} {k}"))
                    .collect();
                if !waits.is_empty() || x.gated > 0 {
                    let mut w = waits.join(" · ");
                    if x.gated > 0 {
                        w.push_str(&format!(" · {} gated", x.gated));
                    }
                    if let Some((id, n)) = x.top_waited.first() {
                        w.push_str(&format!(" · most on node {id} ×{n}"));
                    }
                    rows.push(("waits".to_owned(), w.trim_start_matches(" · ").to_owned()));
                }
                if x.max_ready > 0 {
                    rows.push((
                        "ready".to_owned(),
                        format!(
                            "{:.1} a step on average · at most {}",
                            x.ready_mean, x.max_ready
                        ),
                    ));
                }
                let [bounded, any, viol, undecl, stops, waits] = x.hints;
                if bounded + any > 0 {
                    let red = |n: u64, what: &str| {
                        let t = format!("{n} {what}");
                        if n > 0 { st.red(&t) } else { t }
                    };
                    rows.push((
                        "hints".to_owned(),
                        format!(
                            "{bounded} bounded · {any} any · {} · {} → {stops} stops · {waits} pending waits",
                            red(viol, "violations"),
                            red(undecl, "undeclared")
                        ),
                    ));
                }
                if x.speculations > 0 {
                    rows.push((
                        "ahead".to_owned(),
                        format!(
                            "{} run ahead · {} confirmed · {} run again · {} raises · {} out of fuel",
                            x.speculations, x.confirmations, x.mispredictions, x.raises, x.fuel_outs
                        ),
                    ));
                }
            }
            let head = format!(
                "{}  {}  {}",
                st.bold(&format!("build {builds}")),
                st.cyan(&place),
                st.bold(&ms(r.wall))
            );
            last.borrow_mut().summary = tree(st, head, &rows);
        }
        phitex_tex::timeline::lap("watch: the build's line and summary");
        ui.event(&json!({
            "event": "build",
            "build": builds,
            "why": why,
            "notes": r.notes,
            "edit": r.edit_line.map(|l| format!("{main_rel}:{l}")),
            "ok": !failed,
            "output": written.as_ref().map(|p| p.display().to_string()),
            "pages": r.pages(),
            "pages_changed": changed,
            "nodes_run": nodes,
            "absorbed": r.runs.last().map(|x| x.absorbed),
            "parallel": r.runs.last().map(|x| json!({
                "threads": x.threads,
                "busy_ms": x.busy.as_secs_f64() * 1e3,
                "engine_ms": x.engine_wall.as_secs_f64() * 1e3,
                "critical_path": x.critical_path,
                "peak_running": x.peak_running,
                "batches": x.batches,
                "batches_inline": x.batches_inline,
                "max_batch": x.max_batch,
                "waits_by_kind": x.waits_by_kind.iter().map(|(k, n)| json!({"kind": k, "waits": n})).collect::<Vec<_>>(),
                "top_waited": x.top_waited.iter().map(|(id, n)| json!({"node": id, "waits": n})).collect::<Vec<_>>(),
                "gated": x.gated,
                "hints": {
                    "bounded": x.hints[0],
                    "any": x.hints[1],
                    "violations": x.hints[2],
                    "undeclared": x.hints[3],
                    "stops": x.hints[4],
                    "pending_waits": x.hints[5],
                },
                "ready_mean": x.ready_mean,
                "max_ready": x.max_ready,
                "speculation": {
                    "run": x.speculations,
                    "confirmed": x.confirmations,
                    "mispredicted": x.mispredictions,
                    "raises": x.raises,
                    "fuel_outs": x.fuel_outs,
                },
            })),
            "wall_ms": r.wall.as_secs_f64() * 1e3,
            "errors": r.out.errors,
            "diagnostics": r.engine_diagnostics(),
        }));
        phitex_tex::timeline::lap("watch: the build event written");
        let trep = std::time::Instant::now();
        let tevent = trep;
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
        if phitex_tex::tex_time() {
            eprintln!("watch phases: report {:?}", trep.elapsed());
        }
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
        phitex_tex::timeline::lap("watch: after the event (report, status)");
        if phitex_tex::tex_time() {
            eprintln!("watch phases: after the event {:?}", tevent.elapsed());
        }
        last.borrow_mut().diags = std::mem::take(&mut r.diagnostics);
        drop(r);
        phitex_tex::timeline::lap("watch: the report dropped");
        phitex_tex::timeline::end(&format!("build {builds}"));
    };
    // (what came while a build ran and it could not take: done after it)
    let mut held: VecDeque<(Instant, Event)> = VecDeque::new();
    let mut held_asks: Vec<Ask> = Vec::new();
    // (while the cold build runs: the main file changed on disk is sent
    // to it, everything held for after it)
    let cold_absorb = s.absorb();
    let mut cold_meanwhile = || {
        let Ok((at, e)) = rx.recv() else {
            return true;
        };
        if let Event::Built = e {
            return true;
        }
        if let Event::Files(ps) = &e
            && ps
                .iter()
                .all(|p| p.strip_prefix(&root).unwrap_or(p) == Path::new(&main_rel))
            && let Ok(b) = std::fs::read(&main_file)
        {
            cold_absorb.send(&tex::chars(&b), at);
        }
        held.push_back((at, e));
        false
    };
    rebuild(&mut s, "cold", None, &mut cold_meanwhile);
    let set = |s: &mut Session, buffers: &HashMap<String, String>, file: &str| {
        let text = buffers.get(file).map(|b| tex_text(b));
        if file == main_rel {
            let t = text.unwrap_or_else(|| {
                let t0 = Instant::now();
                let b = std::fs::read(&main_file).unwrap_or_default();
                phitex_tex::timeline::note("watch: the main file read", t0.elapsed());
                let t0 = Instant::now();
                let t = tex::chars_owned(b);
                phitex_tex::timeline::note("watch: its characters", t0.elapsed());
                t
            });
            let t0 = Instant::now();
            s.set_text(t);
            phitex_tex::timeline::note("watch: the text set (shared)", t0.elapsed());
        } else {
            s.set_buffer(file, text.as_deref());
        }
    };
    // (the diff against a past version, and what the viewer asked of it)
    let mut comparing: Option<crate::compare::Compare> = None;
    let mut diff_asks: Vec<Ask> = Vec::new();
    while let Some(first) = held.pop_front().or_else(|| rx.recv().ok()) {
        // (what came meanwhile, a build's time of keystrokes among it, is
        // one edit, timed from the last of them: the wait a writer feels)
        let mut events = vec![first];
        events.extend(held.drain(..));
        while let Ok(e) = rx.recv_timeout(Duration::from_micros(500)) {
            events.push(e);
        }
        let came = events
            .iter()
            .filter(|e| matches!(e.1, Event::Files(_) | Event::Asked | Event::Edited(_)))
            .map(|e| e.0)
            .max()
            .unwrap_or_else(Instant::now);
        phitex_tex::timeline::begin_at(came);
        phitex_tex::timeline::lap("watch: the events gathered");
        let events = events.into_iter().map(|e| e.1);
        let mut why = Vec::new();
        let mut again = false;
        let mut main_changed = false;
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
                    Key::Problems
                        if view.as_ref().is_some_and(View::diff_is_shown)
                            && comparing.is_some() =>
                    {
                        let errs = comparing
                            .as_ref()
                            .map(|c| c.errors(st.color))
                            .unwrap_or_default();
                        if errs.is_empty() {
                            ui.line(&st.green("the diff: no errors"));
                        }
                        for e in errs {
                            ui.line(&e);
                        }
                    }
                    Key::Problems if last.borrow().placed.0 > 0 => {
                        let l = last.borrow();
                        for p in crate::problems::rendered_of(
                            &s,
                            &l.diags,
                            false,
                            st.color,
                            ui.verbose > 0,
                        ) {
                            ui.line(&p);
                        }
                    }
                    Key::Warnings if last.borrow().placed.1 > 0 => {
                        let l = last.borrow();
                        let max = if ui.verbose > 0 { usize::MAX } else { 20 };
                        let placed = crate::problems::rendered_of(
                            &s,
                            &l.diags,
                            true,
                            st.color,
                            ui.verbose > 0,
                        );
                        for p in placed.iter().take(max) {
                            ui.line(p);
                        }
                        if placed.len() > max {
                            ui.line(&st.dim(&format!(
                                "… {} warnings in all (-v shows every one)",
                                placed.len()
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
                        // (the main file read once for all its events)
                        if rel == main_rel && !buffers.contains_key(&rel) {
                            main_changed = true;
                        }
                        why.push(rel);
                        again = true;
                    }
                }
                // (a build's end is taken by the loop that waits on it)
                Event::Built => {}
                Event::Edited(file) => {
                    set(&mut s, &buffers, &file);
                    why.push(format!("{file} (editor)"));
                    again = true;
                }
                Event::Asked => {
                    let mut asks = std::mem::take(&mut held_asks);
                    asks.extend(view.as_ref().map(View::take_asks).unwrap_or_default());
                    for a in asks {
                        let file = match a {
                            Ask::Painted(us) => {
                                let mut l = last.borrow_mut();
                                let d = Duration::from_micros(us);
                                l.screen = Some(d);
                                l.screens.push(d);
                                ui.event(&json!({
                                    "event": "screen",
                                    "build": l.builds,
                                    "ms": d.as_secs_f64() * 1e3,
                                }));
                                show(ui, &l);
                                continue;
                            }
                            Ask::Rebuild => {
                                why.push("asked in the viewer".to_owned());
                                again = true;
                                continue;
                            }
                            Ask::DiffStart { .. }
                            | Ask::DiffShow(_)
                            | Ask::DiffRestyle { .. }
                            | Ask::DiffStop => {
                                diff_asks.push(a);
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
        // (a change the last build took while it ran: built already)
        // (r, or Rebuild in the viewer: built again whatever)
        phitex_tex::timeline::lap("watch: the events taken");
        if main_changed && !buffers.contains_key(&main_rel) {
            set(&mut s, &buffers, &main_rel);
            phitex_tex::timeline::lap("watch: the main file read and set");
        }
        let asked = why.iter().any(|w| w.starts_with("asked"));
        if again && !asked && s.up_to_date() {
            again = false;
        }
        phitex_tex::timeline::lap("watch: whether the last build is up to date");
        if again {
            why.dedup();
            let absorb = s.absorb();
            // (while the build runs: the main file's edits sent to it, the
            // rest held for after it)
            let mut meanwhile = || {
                let Ok((at, e)) = rx.recv() else {
                    return true;
                };
                let send_main = |buffers: &HashMap<String, String>, file: &str| -> bool {
                    if file != main_rel {
                        return false;
                    }
                    let t = buffers.get(file).map_or_else(
                        || {
                            std::fs::read(&main_file)
                                .map(|b| tex::chars(&b))
                                .unwrap_or_default()
                        },
                        |b| tex_text(b),
                    );
                    absorb.send(&t, at);
                    true
                };
                match e {
                    Event::Built => return true,
                    Event::Asked => {
                        for a in view.as_ref().map(View::take_asks).unwrap_or_default() {
                            match a {
                                Ask::SetFile { ref file, ref text } if *file == main_rel => {
                                    buffers.insert(file.clone(), text.clone());
                                }
                                Ask::Edit {
                                    ref file,
                                    start,
                                    end,
                                    ref text,
                                } if *file == main_rel => {
                                    let cur = buffers.get(file).cloned().unwrap_or_else(|| {
                                        std::fs::read(&main_file)
                                            .map(|b| String::from_utf8_lossy(&b).into_owned())
                                            .unwrap_or_default()
                                    });
                                    buffers
                                        .insert(file.clone(), splice_utf16(&cur, start, end, text));
                                }
                                other => {
                                    held_asks.push(other);
                                    continue;
                                }
                            }
                            if send_main(&buffers, &main_rel) {
                                held.push_back((at, Event::Edited(main_rel.clone())));
                            }
                        }
                        if !held_asks.is_empty() {
                            held.push_back((at, Event::Asked));
                        }
                    }
                    Event::Files(ps)
                        if ps.iter().all(|p| {
                            p.strip_prefix(&root).unwrap_or(p) == Path::new(&main_rel)
                        }) && !buffers.contains_key(&main_rel) =>
                    {
                        send_main(&buffers, &main_rel);
                        held.push_back((at, Event::Files(ps)));
                    }
                    e => held.push_back((at, e)),
                }
                false
            };
            rebuild(&mut s, &why.join(", "), Some(came), &mut meanwhile);
        }
        // (the diff: asked for, or diffed again after the edit)
        if let Some(v) = &view {
            let now = crate::compare::Now {
                root: &root,
                buffers: &buffers,
            };
            let edited = again && comparing.as_ref().is_some_and(|c| c.to.is_none());
            for a in std::mem::take(&mut diff_asks) {
                match a {
                    Ask::DiffStart { rev, to } => {
                        let spin = ui.spin(format!("{what}: diff against {rev}"), status.clone());
                        let started = crate::compare::Git::open(&root).and_then(|g| {
                            crate::compare::Compare::start(
                                &g,
                                &rev,
                                to.as_deref(),
                                &now,
                                &main_rel,
                                &s.job,
                                Arc::new(Status::default()),
                            )
                        });
                        spin.done();
                        match started {
                            Ok(c) => {
                                // (its pages in first: then shown)
                                diff_told(ui, v, &ours, Some(&c));
                                comparing = Some(c);
                                v.diff_shown(true);
                            }
                            Err(e) => {
                                ui.warning(&format!("diff: {e}"));
                                v.diff_stopped();
                                ui.event(&json!({"event": "diff", "error": e}));
                            }
                        }
                    }
                    Ask::DiffShow(on) => v.diff_shown(on),
                    Ask::DiffRestyle {
                        markup,
                        subtype,
                        add,
                        del,
                    } => {
                        if let Some(c) = comparing.as_mut() {
                            let look = c.look_from(
                                markup.as_deref(),
                                subtype.as_deref(),
                                add.as_deref(),
                                del.as_deref(),
                            );
                            match crate::compare::Git::open(&root) {
                                Ok(g) => {
                                    c.restyle(look, &now, &g, &main_rel);
                                    diff_told(ui, v, &ours, Some(c));
                                }
                                Err(e) => ui.warning(&format!("diff: {e}")),
                            }
                        }
                    }
                    Ask::DiffStop => {
                        comparing = None;
                        v.diff_stopped();
                        ui.line(&st.dim("diff: stopped"));
                    }
                    _ => {}
                }
            }
            if edited && let Some(c) = comparing.as_mut() {
                let t0 = Instant::now();
                c.edited(&now, &main_rel);
                diff_told(ui, v, &ours, Some(c));
                ui.line(&st.dim(&format!("diff: again in {}", ms(t0.elapsed()))));
            }
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
    fn quit_line_has_the_screen_times() {
        let st = crate::ui::Style {
            color: false,
            tty: false,
        };
        let mut l = Last {
            builds: 3,
            cold: Some(Duration::from_secs(2)),
            ..Last::default()
        };
        assert!(!farewell(st, &l).unwrap().contains("screen"));
        l.screens = [40, 12, 25].map(Duration::from_millis).to_vec();
        let q = farewell(st, &l).unwrap();
        assert!(q.contains("to the screen 12"), "{q}");
        assert!(q.contains("(median 25"), "{q}");
    }

    #[test]
    fn utf16_splices() {
        assert_eq!(splice_utf16("héllo", 1, 2, "e"), "hello");
        assert_eq!(splice_utf16("a😀b", 3, 3, "c"), "a😀cb");
        assert_eq!(splice_utf16("ab", 5, 9, "c"), "abc");
    }
}
