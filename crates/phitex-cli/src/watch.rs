//! `phitex watch`: the session kept alive. A change of a project file on
//! disk, or an editor's unsaved buffer (the viewer's `set_file` and
//! `edit`), is given to the same `Build` as a new version of that file,
//! and the build runs again as an edit (to the fixpoint); the outputs are
//! written and the viewer told which pages changed. On standard input: `r`
//! rebuilds, `q` (or its end) quits.

use std::collections::HashMap;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use crate::modern::{self, Options, Style, Target, ms};
use crate::session::{self, Session};
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

/// How often the project's files are looked at.
const POLL: Duration = Duration::from_millis(20);

/// The extensions of the files a change of rebuilds.
const SOURCES: &[&str] = &[
    "tex", "sty", "cls", "bib", "bst", "def", "cfg", "clo", "fd", "ltx", "ist", "bbx", "cbx", "lbx", "dbx",
    "png", "jpg", "jpeg", "pdf", "eps", "mp", "txt", "dat", "csv", "tikz", "pgf", "inc",
];

pub type Stamp = (u64, SystemTime);

/// The project's source files under `root` (to depth 6, no hidden
/// directories) with their sizes and times.
pub fn scan(root: &Path) -> HashMap<PathBuf, Stamp> {
    let mut out = HashMap::new();
    let mut todo = vec![(root.to_path_buf(), 0)];
    while let Some((d, depth)) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                if depth < 6 {
                    todo.push((p, depth + 1));
                }
            } else if p.extension().is_some_and(|x| SOURCES.contains(&&*x.to_string_lossy().to_ascii_lowercase())) {
                out.insert(p, (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
            }
        }
    }
    out
}

/// The files the watch wrote itself, with their bytes' hashes: a change
/// to one of them that leaves those bytes is the watch's own.
type Ours = Arc<Mutex<HashMap<PathBuf, u64>>>;

fn poller(root: PathBuf, ours: Ours, tx: mpsc::Sender<Event>) {
    let mut last = scan(&root);
    loop {
        std::thread::sleep(POLL);
        let now = scan(&root);
        let mut changed: Vec<PathBuf> = Vec::new();
        for (p, st) in &now {
            if last.get(p) != Some(st) {
                changed.push(p.clone());
            }
        }
        for p in last.keys() {
            if !now.contains_key(p) {
                changed.push(p.clone());
            }
        }
        last = now;
        {
            let ours = ours.lock().unwrap_or_else(PoisonError::into_inner);
            changed.retain(|p| {
                let mine = ours.get(p).or_else(|| p.canonicalize().ok().and_then(|c| ours.get(&c)));
                match mine {
                    Some(&h) => std::fs::read(p).map_or(true, |b| tex::Fnv::new().add(&b).0 != h),
                    None => true,
                }
            });
        }
        if !changed.is_empty() && tx.send(Event::Files(changed)).is_err() {
            return;
        }
    }
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
fn diagnostics(r: &session::Report, log: &str) -> Vec<String> {
    let mut v: Vec<String> = r
        .out
        .errors
        .iter()
        .map(|e| {
            format!(
                "{{\"severity\":\"error\",\"code\":\"tex\",\"message\":{}}}",
                crate::json::quote(crate::format::bare(e))
            )
        })
        .collect();
    for (k, n, first) in crate::warnings::summarize(&r.log_text()) {
        v.push(format!(
            "{{\"severity\":\"warning\",\"code\":\"{k:?}\",\"message\":{},\"detail\":{}}}",
            crate::json::quote(&format!("{n} × {}", first.first().cloned().unwrap_or_default())),
            crate::json::quote(&format!("see {log}"))
        ));
    }
    v
}

/// Watch target `t`; never returns.
#[allow(clippy::too_many_lines)]
pub fn main(t: &Target, o: &Options, st: &Style) -> ! {
    let job = modern::job(t, o).unwrap_or_else(|e| {
        eprintln!("{}: {e}", st.red("error"));
        std::process::exit(2);
    });
    for n in session::waiting_notes(&job) {
        eprintln!("{}: {n}", st.yellow("note"));
    }
    let mut s = Session::new(job, &mut |n: &str| eprintln!("{}", st.dim(&format!("phitex: {n}")))).unwrap_or_else(|e| {
        eprintln!("{}: {e}", st.red("error"));
        std::process::exit(1);
    });
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let main_file = root.join(&s.job.path);
    let main_rel = s.job.path.to_string_lossy().trim_start_matches("./").to_owned();
    let (tx, rx) = mpsc::channel::<Event>();
    // (the viewer: on a terminal unless --no-view; anywhere with --view)
    let want_view = o.view.unwrap_or_else(|| std::io::IsTerminal::is_terminal(&std::io::stdout()));
    let editor = crate::editor::Editor::configured(o.editor.as_deref()).unwrap_or_else(|e| {
        eprintln!("{}: {e}", st.yellow("warning"));
        None
    });
    let view = if want_view {
        match View::start(editor) {
            Ok(v) => {
                let tx = Mutex::new(tx.clone());
                v.on_ask(Box::new(move || {
                    let _ = tx.lock().unwrap_or_else(PoisonError::into_inner).send(Event::Asked);
                }));
                println!("phitex: the viewer is at {}", v.url());
                if o.open && !view::open_browser(v.url()) {
                    eprintln!("{}: could not start a browser", st.yellow("warning"));
                }
                Some(v)
            }
            Err(e) => {
                eprintln!("{}: the viewer: {e}", st.yellow("warning"));
                None
            }
        }
    } else {
        None
    };
    let ours: Ours = Arc::default();
    {
        let (root, ours, tx) = (root.clone(), ours.clone(), tx.clone());
        std::thread::Builder::new()
            .name("phitex-watch-files".into())
            .spawn(move || poller(root, ours, tx))
            .expect("a thread");
    }
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
    let mut rebuild = |s: &mut Session, why: &str| {
        builds += 1;
        if let Some(v) = &view {
            v.building(true);
        }
        let r = s.build();
        // (the poller waits on the lock while the files are written, then
        // knows them as the watch's own)
        let mut mine = ours.lock().unwrap_or_else(PoisonError::into_inner);
        let written = match s.write(&r) {
            Ok(w) => {
                let ours = &mut *mine;
                for (p, h) in &w.files {
                    let p = p.canonicalize().unwrap_or_else(|_| root.join(p));
                    ours.insert(p, *h);
                }
                w.main
            }
            Err(e) => {
                eprintln!("{}: {e}", st.red("error"));
                None
            }
        };
        drop(mine);
        let update = r.runs.first().map(|x| x.wall).unwrap_or_default();
        let mut line = format!(
            "phitex: build {builds} ({why}) in {} (update {:.2} ms; {} run{}); {} page{}, {} kept",
            ms(r.wall),
            update.as_secs_f64() * 1e3,
            r.runs.len(),
            if r.runs.len() == 1 { "" } else { "s" },
            r.pages(),
            if r.pages() == 1 { "" } else { "s" },
            r.out.stats.pages_reused
        );
        if let Some(v) = &view {
            let pdf = match r.main_output() {
                Some(("pdf", b)) => Some(session::shared(b)),
                Some((_, dvi)) => match crate::dvipdf::dvi_to_pdf(dvi, &format!("{}.pdf", s.job.jobname), s.job.pdf_date) {
                    Ok(p) => Some(session::shared(&p)),
                    Err(e) => {
                        eprintln!("{}: the viewer's PDF of the DVI file: {e}", st.yellow("warning"));
                        None
                    }
                },
                None => None,
            };
            let log = s.job.out_dir.join(format!("{}.log", s.job.jobname)).display().to_string();
            let (changed, n) = v.built(&Built {
                pdf,
                history: r.history(),
                ms: r.wall.as_secs_f64() * 1e3,
                diagnostics: diagnostics(&r, &log),
            });
            v.building(false);
            line.push_str(&format!(
                "; viewer: {} of {n} page{} redrawn{}",
                changed.len(),
                if n == 1 { "" } else { "s" },
                if changed.is_empty() { String::new() } else { format!(" ({})", view::ranges(&changed)) }
            ));
        }
        println!("{line}");
        modern::report(t, s, &r, written.as_deref(), &Options { quiet: true, ..Options::default() }, st);
    };
    rebuild(&mut s, "cold");
    let set = |s: &mut Session, buffers: &HashMap<String, String>, file: &str| {
        let text = buffers.get(file).map(|b| tex_text(b));
        if file == main_rel {
            let t = text.unwrap_or_else(|| std::fs::read(&main_file).map(|b| tex::chars(&b)).unwrap_or_default());
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
                Event::Quit => std::process::exit(0),
                Event::Line(l) => match l.trim() {
                    "q" | "quit" => std::process::exit(0),
                    "r" | "" => {
                        again = true;
                        why.push("asked".to_owned());
                    }
                    other => eprintln!("phitex watch: `{other}`: r rebuilds, q quits"),
                },
                Event::Files(ps) => {
                    for p in ps {
                        let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().into_owned();
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
                            Ask::Edit { file, start, end, text } => {
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
