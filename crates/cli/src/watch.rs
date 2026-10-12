//! `phitex watch`: the session kept alive. A change of a project file on
//! disk, or an editor's unsaved buffer (the viewer's `set_file` and
//! `edit`), is given to the same `Build` as a new version of that file,
//! and the build runs again as an edit (to the fixpoint); the outputs are
//! written and the viewer told which pages changed. The files are watched
//! by the system (`notify`: inotify, FSEvents, kqueue). On standard input:
//! `r` rebuilds, `q` (or its end) quits.

use std::collections::HashMap;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher as _};
use serde_json::{Value, json};

use crate::modern::{self, Options, Target, Ui, ms};
use crate::session::{self, Session, Status};
use crate::tex;
use crate::view::{self, Ask, Built, View};

/// What wakes the watch.
enum Event {
    /// Files of the project changed on disk.
    Files(Vec<PathBuf>),
    /// A line on standard input.
    Line(String),
    /// The viewer asked for something.
    Asked,
    Quit,
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
                let tx = Mutex::new(tx.clone());
                v.on_ask(Box::new(move || {
                    let _ = tx
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send(Event::Asked);
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
    {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("phitex-watch-stdin".into())
            .spawn(move || {
                for l in std::io::stdin().lock().lines() {
                    match l {
                        Ok(l) => {
                            if tx.send(Event::Line(l)).is_err() {
                                return;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx.send(Event::Quit);
            })
            .expect("a thread");
    }
    drop(tx);
    // (the editor's buffers, as Unicode text, by file name)
    let mut buffers: HashMap<String, String> = HashMap::new();
    let mut builds = 0usize;
    // (the last output written without errors, and its PDF for the viewer)
    let mut last_good: Option<PathBuf> = None;
    let mut last_pdf = None;
    let mut last_hashes: Vec<u64> = Vec::new();
    // (`viewer = "zathura"`: --open starts it on the output, once)
    let open_cmd = t.viewer.clone().filter(|_| o.open);
    let mut started = false;
    let mut rebuild = |s: &mut Session, why: &str| {
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
                        diagnostics: vec![
                            json!({"severity": "error", "code": "engine", "message": first}),
                        ],
                    });
                    v.building(false);
                }
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
            last_good = Some(out.clone());
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
            facts.push(if last_good.is_some() {
                "last good output kept".to_owned()
            } else {
                "nothing written".to_owned()
            });
        } else {
            facts.push(format!("{nodes} nodes run"));
            facts.push(if builds == 1 {
                format!(
                    "{} page{}",
                    r.pages(),
                    if r.pages() == 1 { "" } else { "s" }
                )
            } else if changed.is_empty() {
                format!("no page of {} changed", r.pages())
            } else {
                format!(
                    "{} of {} page{} changed ({})",
                    changed.len(),
                    r.pages(),
                    if r.pages() == 1 { "" } else { "s" },
                    view::ranges(changed)
                )
            });
        }
        facts.push(st.bold(&ms(r.wall)));
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
        // (--open with a viewer program: started on the first output)
        if let (Some(cmd), Some(out), false) = (&open_cmd, &last_good, started) {
            started = true;
            if let Err(e) = view::open_with(cmd, out) {
                ui.warning(&format!("can't start {cmd}: {e}"));
            }
        }
    };
    rebuild(&mut s, "cold");
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
        // (what came meanwhile is one edit)
        let mut events = vec![first];
        while let Ok(e) = rx.recv_timeout(Duration::from_millis(2)) {
            events.push(e);
        }
        let mut why = Vec::new();
        let mut again = false;
        for e in events {
            match e {
                Event::Quit => {
                    modern::cache_summary(t, ui);
                    std::process::exit(0);
                }
                Event::Line(l) => match l.trim() {
                    "q" | "quit" => {
                        modern::cache_summary(t, ui);
                        std::process::exit(0);
                    }
                    "r" | "" => {
                        again = true;
                        why.push("asked".to_owned());
                    }
                    other => ui.line(&format!("phitex watch: `{other}`: r rebuilds, q quits")),
                },
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
            rebuild(&mut s, &why.join(", "));
        }
    }
    modern::cache_summary(t, ui);
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_splices() {
        assert_eq!(splice_utf16("héllo", 1, 2, "e"), "hello");
        assert_eq!(splice_utf16("a😀b", 3, 3, "c"), "a😀cb");
        assert_eq!(splice_utf16("ab", 5, 9, "c"), "abc");
    }
}
