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
//! message the terminal shows is the viewer's too (its console). With
//! `--lsp`, standard input and output are an editor's language client's
//! instead (`lsp.rs`): its buffers are asked as the viewer's are, its
//! problems published after each build, its cursor shown on the pages.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher as _};
use serde_json::{Value, json};

use crate::keys::{self, Key};
use crate::lsp::Lsp;
use crate::modern::{self, Options, Target, Ui, ms};
use crate::session::{self, Session, Status};
use crate::tex;
use crate::ui::pad;
use crate::view::{self, Ask, Built, View};
use phitex_live::watch::{Applied, Edits, diagnostics, splice_utf16, tex_text};

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
    /// The exact PDF file written over the updated one (when idle),
    /// that of build `.0`: whether it was, or why not.
    Compacted(usize, Result<bool, String>),
    /// The editor's settings changed (`--lsp`): a new session where they
    /// change the threads or the main file.
    Configure(phitex_live::lsp::Settings),
}

/// What the watch keeps from one session to the next (the editor's
/// settings changed: [`Event::Configure`]): the editor, the viewer and
/// its address, the events, standard input's reader.
struct Kept {
    lsp: Option<Arc<Lsp>>,
    tx: mpsc::Sender<(Instant, Event)>,
    rx: Option<mpsc::Receiver<(Instant, Event)>>,
    /// The viewer (none: `--no-view`), once the first session made it.
    view: Option<View>,
    view_made: bool,
    building: Arc<AtomicBool>,
    told: Arc<AtomicBool>,
    /// Whether standard input is read (by the first session).
    reading: bool,
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

/// Whether the system says when a file's writer closes it (inotify's
/// close-write): a change is then read once its save ends.
const CLOSES: bool = cfg!(any(target_os = "linux", target_os = "android"));

/// How long a file written and not closed waits before it is read
/// anyway (a writer that keeps it open).
const WRITE_WAIT: Duration = Duration::from_millis(500);

/// `changed` without the watch's own writes (`ours`: a change that
/// leaves the bytes it wrote).
fn not_ours(ours: &Ours, changed: &mut Vec<PathBuf>) {
    let ours = ours.lock().unwrap_or_else(PoisonError::into_inner);
    changed.retain(|p| {
        let mine = ours
            .get(p)
            .or_else(|| p.canonicalize().ok().and_then(|c| ours.get(&c)));
        match mine {
            Some(&tex::ANY_BYTES) => false,
            Some(&h) => tex::fingerprint_file(p) != Some(h),
            None => true,
        }
    });
}

/// The system's file events under `root`, as the project's changed
/// sources (the watch's own writes left out). Where the system says when
/// a writer closes a file ([`CLOSES`]), a file created or written is a
/// change once closed, not at each write of its save: an editor that
/// saves in place truncates the file first, and a build of it read
/// between would be one of a cut text. A file removed or renamed away is
/// a change only when nothing writes it again within [`WRITE_WAIT`]: an
/// editor that saves by renaming the old file to a backup and writing a
/// new one (vim's `backupcopy=no`, many others) leaves no file at that
/// name for a moment, and a build read then is one of an empty text.
fn watcher(
    root: &Path,
    ours: Ours,
    tx: mpsc::Sender<(Instant, Event)>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RenameMode};
    let r = root.to_path_buf();
    // (the files written and not closed yet)
    let writing: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
    let mut w = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
        let Ok(ev) = ev else { return };
        // (a rename's pair: its two halves come as `From` and `To` too)
        if matches!(
            ev.kind,
            EventKind::Modify(ModifyKind::Name(RenameMode::Both))
        ) {
            return;
        }
        let closed = CLOSES
            && matches!(
                ev.kind,
                EventKind::Access(AccessKind::Close(AccessMode::Write))
            );
        if !closed
            && !matches!(
                ev.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            )
        {
            return;
        }
        let mut changed: Vec<PathBuf> = ev.paths.into_iter().filter(|p| source(&r, p)).collect();
        if CLOSES {
            let written = matches!(
                ev.kind,
                EventKind::Create(CreateKind::File | CreateKind::Any)
                    | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any)
            );
            // (gone from its name: a change once nothing writes it again
            // within WRITE_WAIT, as a file written and not closed)
            let gone = matches!(
                ev.kind,
                EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From))
            );
            let mut open = writing.lock().unwrap_or_else(PoisonError::into_inner);
            if written || gone {
                // (read after WRITE_WAIT if its writer keeps it open)
                for p in changed.drain(..) {
                    if open.insert(p.clone()) {
                        let (writing, ours, tx) = (writing.clone(), ours.clone(), tx.clone());
                        std::thread::spawn(move || {
                            std::thread::sleep(WRITE_WAIT);
                            let late = writing
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .remove(&p);
                            let mut changed = vec![p];
                            if late {
                                not_ours(&ours, &mut changed);
                                if !changed.is_empty() {
                                    let _ = tx.send((Instant::now(), Event::Files(changed)));
                                }
                            }
                        });
                    }
                }
            } else if closed {
                // (a close of a file not written changes nothing)
                changed.retain(|p| open.remove(p));
            } else if matches!(ev.kind, EventKind::Modify(ModifyKind::Name(_))) {
                // (a whole file renamed to the name: read now)
                for p in &changed {
                    open.remove(p);
                }
            } else {
                // (its attributes changed, a touch: read now, but a file
                // still written is read once closed)
                changed.retain(|p| !open.contains(p));
            }
        }
        not_ours(&ours, &mut changed);
        if !changed.is_empty() {
            let _ = tx.send((Instant::now(), Event::Files(changed)));
        }
    })?;
    w.watch(root, RecursiveMode::Recursive)?;
    Ok(w)
}

/// The next event, the receiver borrowed uniquely: a closure that calls
/// this holds `&mut Receiver` (which may cross to another thread), where
/// one calling `rx.recv()` holds `&Receiver` (which may not).
fn recv_unique<T>(rx: &mut mpsc::Receiver<T>) -> Result<T, mpsc::RecvError> {
    rx.recv()
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
    /// `s` and `w` toggle: while on, the summary (the warnings) are shown
    /// after every build, not only when asked.
    pin_summary: bool,
    pin_warnings: bool,
}

/// The last build's warnings, as `w` shows them: at their places where
/// the diagnostics have them, else summarised from the log by kind.
fn show_warnings(ui: &crate::ui::Ui, st: crate::ui::Style, s: &Session, l: &Last) {
    if l.placed.1 > 0 {
        let max = if ui.verbose > 0 { usize::MAX } else { 20 };
        let placed = crate::problems::rendered_of(s, &l.diags, true, st.color, ui.verbose > 0);
        for p in placed.iter().take(max) {
            ui.line(p);
        }
        if placed.len() > max {
            ui.line(&st.dim(&format!(
                "… {} warnings in all (-v shows every one)",
                placed.len()
            )));
        }
        return;
    }
    match &l.problems {
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
    }
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

/// Watch target `t` (`lsp`: for the editor that initialized it, on
/// standard input and output); never returns. The editor's settings may
/// start a new session ([`Event::Configure`]): the viewer, its address
/// and the editor's connection are kept.
pub fn main(t: &Target, o: &Options, ui: &Ui, lsp: Option<Lsp>) -> ! {
    let (tx, rx) = mpsc::channel::<(Instant, Event)>();
    let lsp = lsp.map(Arc::new);
    if let Some(l) = &lsp {
        let tx = Mutex::new(tx.clone());
        l.on_configure(Box::new(move |s| {
            let _ = tx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send((Instant::now(), Event::Configure(s)));
        }));
    }
    let mut kept = Kept {
        lsp,
        tx,
        rx: Some(rx),
        view: None,
        view_made: false,
        building: Arc::default(),
        told: Arc::default(),
        reading: false,
    };
    let (mut t, mut o) = (t.clone(), o.clone());
    loop {
        (t, o) = session(&t, &o, ui, &mut kept);
    }
}

/// The target and options of settings `s` over `t` and `o`, where they
/// change the session (its threads, its main file), and its job can be
/// made (else the editor is told why, and none).
fn reconfigured(
    t: &Target,
    o: &Options,
    s: &phitex_live::lsp::Settings,
    lsp: Option<&Lsp>,
) -> Option<(Target, Options)> {
    let mut o2 = o.clone();
    if let Some(n) = s.threads {
        o2.threads = Some(n);
    }
    if let Some(m) = &s.main {
        o2.file = Some(m.clone());
    }
    let same_main = |a: &str, b: &str| {
        let c = |f: &str| {
            Path::new(f)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(f))
        };
        c(a) == c(b)
    };
    let said = |e: &str| {
        if let Some(l) = lsp {
            l.server()
                .show_message(1, &format!("PhiTeX: the settings: {e}"));
        }
    };
    let t2 = match modern::resolve(&o2) {
        Ok(t2) => t2,
        Err(e) => {
            said(&e);
            return None;
        }
    };
    if o2.threads == o.threads && same_main(&t2.file, &t.file) {
        return None;
    }
    match modern::job(&t2, &o2) {
        Ok(_) => Some((t2, o2)),
        Err(e) => {
            said(&e);
            None
        }
    }
}

/// One session of the watch, on target `t`: its builds until the watch
/// ends, or the editor's settings call for another (its target and
/// options).
#[allow(clippy::too_many_lines)]
fn session(t: &Target, o: &Options, ui: &Ui, kept: &mut Kept) -> (Target, Options) {
    let st = ui.st;
    let lsp = kept.lsp.clone();
    let mut job = modern::job(t, o).unwrap_or_else(|e| {
        if let Some(l) = &lsp {
            l.refuse(&e);
        }
        ui.error(&e);
        std::process::exit(crate::EXIT_FAILED);
    });
    if let Some(l) = &lsp {
        l.accept(
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            job.path.to_string_lossy().trim_start_matches("./"),
        );
    }
    // (the banner: what is built into what, by which engine)
    if !ui.quiet {
        let threads = session::threads(job.threads);
        ui.line(&format!(
            "{} {}  {} → {}  {}",
            st.bold("PhiTeX"),
            st.dim(env!("CARGO_PKG_VERSION")),
            st.bold(&t.file),
            st.bold(&format!("{}.{}", job.jobname, job.engine.output.ext())),
            st.dim(&format!(
                "{} · {threads} thread{}",
                job.engine.name,
                if threads == 1 { "" } else { "s" }
            ))
        ));
    }
    for n in session::waiting_notes(&job) {
        ui.line(&format!("{}: {n}", st.yellow("note")));
    }
    // (a watch's readers guess past a long piece rather than wait on its
    // learned fields: an edit reruns it with the same values, and readers
    // that waited would wait for nothing; `--wait-long` is the one-shot
    // build's, learned there)
    job.wait_long = false;
    let status = Arc::new(Status::default());
    let what = modern::what(t, &job);
    let spin = ui.spin(what.clone(), status.clone());
    let mut s = Session::new(job, status.clone(), &mut |n: &str| ui.note(n)).unwrap_or_else(|e| {
        ui.error(&e);
        if let Some(l) = &lsp {
            l.refuse(&e);
        }
        std::process::exit(crate::EXIT_FAILED);
    });
    spin.done();
    // (no log laid out after a build; the PDF file appended an update
    // section after each edit, the exact file written when idle, but
    // with `--exact`)
    s.watching = true;
    if !o.exact_file {
        s.set_updates(Some(Arc::default()));
    }
    // (since when the PDF file is not the exact one: written when the
    // editor has been idle for `o.idle`)
    let idle_since: std::cell::Cell<Option<Instant>> = std::cell::Cell::new(None);
    // (the last build's origins still to be made: after it is announced,
    // once no event waits; never on the typing path)
    let origins_due = std::cell::Cell::new(false);
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let main_file = root.join(&s.job.path);
    let main_rel = s
        .job
        .path
        .to_string_lossy()
        .trim_start_matches("./")
        .to_owned();
    let tx = kept.tx.clone();
    let mut rx = kept.rx.take().expect("the events, between sessions");
    // (a terminal editor, or an unknown one: the viewer shows the place
    // instead of opening it, said with -v only: the user chose the editor
    // and knows where it runs)
    let editor = crate::editor::Editor::configured(t.editor.as_deref()).unwrap_or_else(|e| {
        if ui.verbose > 0 {
            ui.line(&st.dim(&format!(
                "editor `{e}`: the viewer shows a clicked place instead of opening it (--editor code, nvim, emacsclient, …)"
            )));
        }
        None
    });
    // (the viewer: made by the first session, kept by the others)
    let view: Option<View> = if std::mem::replace(&mut kept.view_made, true) {
        kept.view.take()
    } else if o.view {
        match View::start(editor, o.port) {
            Ok(v) => {
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
                    st.dim(if lsp.is_some() {
                        "(phitex.openViewer opens it)"
                    } else {
                        "(o opens it)"
                    })
                ));
                if let Some(l) = &lsp {
                    l.viewer(v.url());
                    // (a double-click's place shown by the editor itself)
                    let l = l.clone();
                    v.on_source(Box::new(move |f, line, col| l.show(f, line, col)));
                }
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
    // (the terminal's messages: the viewer's console, the editor's log)
    {
        let console = view.as_ref().map(View::console);
        let log = lsp.as_ref().map(|l| l.console());
        if console.is_some() || log.is_some() {
            ui.tap(Box::new(move |item: Value| {
                if let Some(l) = &log {
                    l(&item);
                }
                if let Some(c) = &console {
                    c(item);
                }
            }));
        }
    }
    // (the build's progress records: the viewer's `progress` event, the
    // editor's `$/progress` and `phitex/status`)
    {
        let shown = view.as_ref().map(View::viewer);
        let server = lsp.as_ref().map(|l| l.server().clone());
        if shown.is_some() || server.is_some() {
            status.meter().listen(Box::new(move |p| {
                if let Some(v) = &shown {
                    v.progress(p);
                }
                if let Some(l) = &server {
                    l.progress(p);
                }
            }));
        }
    }
    if let Some(l) = &lsp {
        let wake = Mutex::new(tx.clone());
        l.on_ask(Box::new(move || {
            let _ = wake
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send((Instant::now(), Event::Asked));
        }));
    }
    // (what the viewer and the editor asked, taken together)
    let take_asks = || {
        let mut a = view.as_ref().map(|v| v.take_asks()).unwrap_or_default();
        a.extend(lsp.as_ref().map(|l| l.take_asks()).unwrap_or_default());
        a
    };
    let owned = |f: &str| lsp.as_ref().is_some_and(|l| l.owns(f));
    let ours: Ours = Arc::default();
    // (kept for the watch's life: dropping it stops the events)
    let _files = watcher(&root, ours.clone(), tx.clone()).unwrap_or_else(|e| {
        ui.error(&format!("can't watch {}: {e}", root.display()));
        std::process::exit(crate::EXIT_FAILED);
    });
    let raw = lsp.is_none() && !ui.json && keys::raw();
    let building = kept.building.clone();
    let told = kept.told.clone();
    // (standard input read by the first session's thread, for them all)
    if !std::mem::replace(&mut kept.reading, true) {
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
        match &lsp {
            // (the editor's `exit`, or its end: as `q`)
            Some(l) => l.serve(Box::new(move || {
                if !here.answer(Key::Quit) {
                    let _ = tx.send((Instant::now(), Event::Key(Key::Quit)));
                }
            })),
            None => {
                std::thread::Builder::new()
                    .name("phitex-watch-stdin".into())
                    .spawn(move || read_keys(&tx, raw, &here))
                    .expect("a thread");
            }
        }
    }
    // (the build's thread's end, sent to the loop that waits on it)
    let built = Mutex::new(tx.clone());
    // (the exact file's write when idle, on a thread of its own, says
    // when it is done)
    let compacted = tx.clone();
    drop(tx);
    if raw {
        ui.keys(keys::BAR);
    }
    let last = RefCell::new(Last::default());
    let quit = |ui: &Ui, s: &mut Session| -> ! {
        // (the records the builds changed, stored before the watch ends;
        // the exact PDF file and, with `--log`, the log)
        s.flush_records();
        if let Some(u) = &s.updates()
            && let Err(e) = session::compact(u)
        {
            ui.error(&e);
        }
        if o.log
            && let Err(e) = s.write_log()
        {
            ui.error(&e);
        }
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
    let mut edits = Edits::new(&main_rel, &main_file);
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
                       meanwhile: &mut (dyn FnMut() -> bool + Send)| {
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
        if let Some(l) = &lsp {
            l.building();
        }
        if let Some(v) = &view {
            v.building(true);
        }
        let spin = ui.spin(format!("{what}: build {builds} ({why})"), status.clone());
        // (a build that fails inside the engine — a panic: the pieces did
        // not settle — is reported, and the watch goes on waiting)
        // (the build on this thread, the same one every build: what the
        // engine keeps per thread, its tables and caches, stays warm from
        // one build to the next; the events taken meanwhile on a helper)
        let ran = std::thread::scope(|sc| {
            let pump = sc.spawn(|| while !meanwhile() {});
            let r = crate::ui::catch(|| s.build());
            let _ = built
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send((Instant::now(), Event::Built));
            let _ = pump.join();
            r
        });
        // (the records the build changed, stored as it settles: a no-op
        // unless a load changed them)
        s.flush_records();
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
                let b = Built {
                    pdf: last_pdf.clone(),
                    hashes: last_hashes.clone(),
                    history: 2,
                    ms: 0.0,
                    since_ms: None,
                    origins: None,
                    changed: None,
                    diagnostics: vec![
                        json!({"severity": "error", "code": "engine", "message": first}),
                    ],
                };
                if let Some(v) = &view {
                    v.built(&b);
                    v.building(false);
                }
                if let Some(l) = &lsp {
                    l.built(&b);
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
        // (an engine failure, `Report::internal`: nothing written, the
        // last good build's files kept)
        let written = match if r.internal.is_some() {
            Err(String::new())
        } else {
            s.write(&r, true)
        } {
            Ok(w) => {
                let ours = &mut *mine;
                for (p, h) in &w.files {
                    let p = p.canonicalize().unwrap_or_else(|_| root.join(p));
                    ours.insert(p, *h);
                }
                w.main
            }
            Err(e) => {
                if !e.is_empty() {
                    ui.error(&e);
                }
                None
            }
        };
        drop(mine);
        if let Some(u) = &s.updates() {
            let exact = u.lock().unwrap_or_else(PoisonError::into_inner).exact;
            idle_since.set((!exact || o.log).then(Instant::now));
        } else if o.log {
            idle_since.set(Some(Instant::now()));
        }
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
        if view.is_some() || lsp.is_some() {
            if view.is_some() {
                last_pdf = match phitex_live::watch::viewer_pdf(s, &r, || {
                    written.as_ref().and_then(|p| std::fs::read(p).ok())
                }) {
                    Ok(p) => p,
                    Err(e) => {
                        ui.warning(&format!("the viewer's PDF of the DVI file: {e}"));
                        None
                    }
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
            let b = Built {
                pdf: last_pdf.clone(),
                hashes: last_hashes.clone(),
                history: r.history(),
                ms: r.wall.as_secs_f64() * 1e3,
                since_ms: since.map(|t| t.elapsed().as_secs_f64() * 1e3),
                origins: None,
                changed: None,
                diagnostics: diagnostics(s, &r, &log),
            };
            origins_due.set(true);
            if let Some(v) = &view {
                v.built(&b);
                v.building(false);
            }
            if let Some(l) = &lsp {
                l.built(&b);
                // (what phitex.savePdf copies: the PDF file, else a DVI's
                // PDF made for the viewer)
                let pdf = match &written {
                    Some(p) if p.extension().is_some_and(|e| e == "pdf") => {
                        Some(crate::lsp::Pdf::File(p.clone()))
                    }
                    _ => last_pdf.clone().map(crate::lsp::Pdf::Bytes),
                };
                if pdf.is_some() {
                    l.set_pdf(pdf);
                }
            }
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
                // (how parallel the build was, in three facts: the cores
                // its runs kept busy on average, the runs made for the
                // nodes that ran (more runs than nodes: steps run ahead of
                // their inputs and run again), and the time of the runs
                // thrown away; a build too short to measure says its
                // threads and runs)
                let mut p = match x.speedup().filter(|_| x.engine_wall.as_millis() >= 50) {
                    Some(c) => format!("{c:.1} of {} cores busy", x.threads),
                    None => format!(
                        "{} thread{}",
                        x.threads,
                        if x.threads == 1 { "" } else { "s" }
                    ),
                };
                if x.runs > 0 {
                    #[allow(clippy::cast_precision_loss, reason = "a ratio for display")]
                    let k = x.runs as f64 / x.commands_run.max(1) as f64;
                    p.push_str(&format!(" · {} runs for {} nodes", x.runs, x.commands_run));
                    if k >= 1.05 {
                        p.push_str(&format!(" ({k:.1}×)"));
                    }
                }
                if !x.wasted.is_zero() && !x.busy.is_zero() {
                    p.push_str(&format!(" · {} of {} run again", ms(x.wasted), ms(x.busy)));
                }
                rows.push(("parallel".to_owned(), p));
                if x.rss_kb > 0 {
                    #[allow(clippy::cast_precision_loss, reason = "a display of bytes")]
                    let gb = |b: u64| -> String {
                        let g = b as f64 / f64::from(1u32 << 30);
                        if g >= 1.0 {
                            format!("{g:.1} GB")
                        } else {
                            format!("{:.0} MB", b as f64 / f64::from(1u32 << 20))
                        }
                    };
                    rows.push((
                        "memory".to_owned(),
                        format!(
                            "{} resident · {} of values · {} collection{}",
                            gb(x.rss_kb << 10),
                            gb(x.values_bytes),
                            x.collections,
                            if x.collections == 1 { "" } else { "s" }
                        ),
                    ));
                }
                if x.batches > 0 {
                    rows.push((
                        "batches".to_owned(),
                        format!(
                            "{} · {} of one job · largest {}",
                            x.batches, x.batches_inline, x.max_batch
                        ),
                    ));
                }
                if x.max_ready > 0 {
                    rows.push((
                        "ready".to_owned(),
                        format!("at most {} a step", x.max_ready),
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
                            "{} run ahead · {} confirmed · {} run again · {} out of fuel",
                            x.speculations, x.confirmations, x.mispredictions, x.fuel_outs
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
        // (the panels `s` and `w` pinned: shown after every build, so the
        // keys need not be pressed again)
        {
            let l = last.borrow();
            if l.pin_summary {
                for line in &l.summary {
                    ui.line(line);
                }
            }
            if l.pin_warnings {
                show_warnings(ui, st, s, &l);
            }
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
                "hints": {
                    "bounded": x.hints[0],
                    "any": x.hints[1],
                    "violations": x.hints[2],
                    "undeclared": x.hints[3],
                    "stops": x.hints[4],
                    "pending_waits": x.hints[5],
                },
                "max_ready": x.max_ready,
                "speculation": {
                    "run": x.speculations,
                    "confirmed": x.confirmations,
                    "mispredicted": x.mispredictions,
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
        let Ok((at, e)) = recv_unique(&mut rx) else {
            return true;
        };
        if let Event::Built = e {
            return true;
        }
        if let Event::Files(ps) = &e
            && !owned(&main_rel)
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
    // (the diff against a past version, and what the viewer asked of it)
    let mut comparing: Option<crate::compare::Compare> = None;
    let mut diff_asks: Vec<Ask> = Vec::new();
    // (no event for a while: the session is idle, the exact PDF file and,
    // with `--log`, the log written then; the records are stored as each
    // build settles)
    loop {
        // (nothing waits: the last build's origins, made now)
        if origins_due.get() && held.is_empty() {
            match rx.try_recv() {
                Ok(e) => held.push_back(e),
                Err(mpsc::TryRecvError::Empty) => {
                    origins_due.set(false);
                    if let Some(v) = &view {
                        v.set_origins(s.origins(), None);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        let wait = idle_since.get().map_or(Duration::from_secs(2), |t| {
            o.idle.saturating_sub(t.elapsed())
        });
        let first = match held.pop_front() {
            Some(e) => e,
            None => match rx.recv_timeout(wait) {
                Ok(e) => e,
                Err(mpsc::RecvTimeoutError::Timeout) if idle_since.take().is_some() => {
                    // (idle: the exact PDF file written on a thread of
                    // its own, over the updated one; with `--log`, the
                    // log beside it)
                    if o.log
                        && let Err(e) = s.write_log()
                    {
                        ui.error(&e);
                    }
                    if let Some(u) = s.updates() {
                        let tx = compacted.clone();
                        let b = last.borrow().builds;
                        std::thread::Builder::new()
                            .name("phitex-watch-exact".into())
                            .spawn(move || {
                                let r = session::compact(&u);
                                let _ = tx.send((Instant::now(), Event::Compacted(b, r)));
                            })
                            .expect("a thread");
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
        };
        // (what came meanwhile, a build's time of keystrokes among it, is
        // one edit, timed from the last of them: the wait a writer feels)
        // (the events of one save come together, from one read of the
        // system's queue: 100 µs gathers them; a later edit is taken by
        // the build while it runs, `absorb`, or builds again after it)
        let mut events = vec![first];
        events.extend(held.drain(..));
        while let Ok(e) = rx.recv_timeout(Duration::from_micros(100)) {
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
        // (the editor's settings: another session, where they change it)
        let mut next = None;
        for e in events {
            match e {
                Event::Configure(c) => {
                    if let Some(n) = reconfigured(t, o, &c, lsp.as_deref()) {
                        next = Some(n);
                    }
                }
                Event::Key(k) => match k {
                    Key::Quit => quit(ui, &mut s),
                    Key::Rebuild => {
                        // (every input read again from disk: a change made
                        // outside that no event told)
                        s.reread_all();
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
                        if view.as_ref().is_some_and(|v| v.diff_is_shown())
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
                    Key::Warnings => {
                        let on = {
                            let mut l = last.borrow_mut();
                            l.pin_warnings = !l.pin_warnings;
                            l.pin_warnings
                        };
                        if on {
                            show_warnings(ui, st, &s, &last.borrow());
                            ui.line(&st.dim("warnings shown after every build now (w hides them)"));
                        } else {
                            ui.line(&st.dim("warnings hidden (w shows them)"));
                        }
                    }
                    Key::Problems => match &last.borrow().problems {
                        Some((errors, _)) if errors.is_empty() => {
                            ui.line(&st.green("no errors"));
                        }
                        Some((errors, log)) => crate::problems::show(ui, errors, log, &t.file),
                        None => ui.line("phitex watch: no build yet"),
                    },
                    Key::Summary => {
                        let on = {
                            let mut l = last.borrow_mut();
                            l.pin_summary = !l.pin_summary;
                            l.pin_summary
                        };
                        if on {
                            let l = last.borrow();
                            if l.summary.is_empty() {
                                ui.line("phitex watch: no build yet");
                            }
                            for line in &l.summary {
                                ui.line(line);
                            }
                            ui.line(&st.dim("stats shown after every build now (s hides them)"));
                        } else {
                            ui.line(&st.dim("stats hidden (s shows them)"));
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
                        // (a file open in an LSP editor is its buffer's
                        // until closed: the disk's changes not read)
                        if owned(&rel) {
                            continue;
                        }
                        // (a buffer the editor saved is the file again)
                        if let Some(b) = edits.buffers.get(&rel)
                            && std::fs::read(&p).is_ok_and(|d| d == b.as_bytes())
                        {
                            edits.buffers.remove(&rel);
                            edits.set(&mut s, &rel);
                        }
                        // (the main file read once for all its events;
                        // another file the build reads again from disk)
                        if !edits.buffers.contains_key(&rel) {
                            if rel == main_rel {
                                main_changed = true;
                            } else {
                                edits.set(&mut s, &rel);
                            }
                        }
                        why.push(rel);
                        again = true;
                    }
                }
                // (a build's end is taken by the loop that waits on it)
                Event::Built => {}
                Event::Compacted(_, Ok(false)) => {}
                Event::Compacted(b, Ok(true)) => {
                    let out = last.borrow().good.clone();
                    ui.event(&json!({
                        "event": "written",
                        "exact": true,
                        "build": b,
                        "output": out.as_ref().map(|p| p.display().to_string()),
                    }));
                    if let (Some(v), Some(p)) = (&view, &out)
                        && let Ok(b) = std::fs::read(p)
                    {
                        v.pdf_replaced(Some(session::shared(&b)));
                    }
                }
                Event::Compacted(_, Err(e)) => ui.error(&e),
                Event::Edited(file) => {
                    edits.set(&mut s, &file);
                    why.push(format!("{file} (editor)"));
                    again = true;
                }
                Event::Asked => {
                    let mut asks = std::mem::take(&mut held_asks);
                    asks.extend(take_asks());
                    for a in asks {
                        match edits.apply(&mut s, view.as_deref(), a) {
                            Applied::Painted(us) => {
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
                            }
                            Applied::Rebuild => {
                                why.push("asked in the viewer".to_owned());
                                again = true;
                            }
                            Applied::Cold => {
                                why.push("asked: a cold build".to_owned());
                                again = true;
                            }
                            Applied::Diff(a) => diff_asks.push(a),
                            Applied::Edited(file) => {
                                why.push(format!("{file} (editor)"));
                                again = true;
                            }
                            Applied::Nothing => {}
                        }
                    }
                }
            }
        }
        if let Some(n) = next {
            // (the records this session learned stored; the next one
            // builds cold, on what the viewer and the editor still show)
            s.flush_records();
            if let Some(u) = &s.updates()
                && let Err(e) = session::compact(u)
            {
                ui.error(&e);
            }
            ui.line(&st.dim("phitex watch: the editor's settings changed: a new session"));
            kept.rx = Some(rx);
            kept.view = view;
            return n;
        }
        // (a change the last build took while it ran: built already)
        // (r, or Rebuild in the viewer: built again whatever)
        phitex_tex::timeline::lap("watch: the events taken");
        if main_changed && !edits.buffers.contains_key(&main_rel) {
            edits.set(&mut s, &main_rel);
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
                let Ok((at, e)) = recv_unique(&mut rx) else {
                    return true;
                };
                let send_main = |buffers: &HashMap<String, String>, file: &str| -> bool {
                    if file != main_rel {
                        return false;
                    }
                    // (a main file not there to read is not sent as an
                    // empty text)
                    let t = match buffers.get(file) {
                        Some(b) => tex_text(b),
                        None => match std::fs::read(&main_file) {
                            Ok(b) => tex::chars(&b),
                            Err(_) => return false,
                        },
                    };
                    absorb.send(&t, at);
                    true
                };
                match e {
                    Event::Built => return true,
                    Event::Asked => {
                        for a in take_asks() {
                            match a {
                                Ask::SetFile { ref file, ref text } if *file == main_rel => {
                                    edits.buffers.insert(file.clone(), text.clone());
                                }
                                Ask::Edit {
                                    ref file,
                                    start,
                                    end,
                                    ref text,
                                } if *file == main_rel => {
                                    let cur =
                                        edits.buffers.get(file).cloned().unwrap_or_else(|| {
                                            std::fs::read(&main_file)
                                                .map(|b| String::from_utf8_lossy(&b).into_owned())
                                                .unwrap_or_default()
                                        });
                                    edits
                                        .buffers
                                        .insert(file.clone(), splice_utf16(&cur, start, end, text));
                                }
                                other => {
                                    held_asks.push(other);
                                    continue;
                                }
                            }
                            if send_main(&edits.buffers, &main_rel) {
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
                        }) && !edits.buffers.contains_key(&main_rel) =>
                    {
                        send_main(&edits.buffers, &main_rel);
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
                buffers: &edits.buffers,
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
    quit(ui, &mut s)
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
}
