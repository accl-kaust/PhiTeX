//! LuaLaTeX: the operating system's `lualatex` (TeX Live's, from `PATH`)
//! run as an external process, **not on the graph and not incremental**:
//! every build runs the whole document again. phitex drives it as latexmk
//! would: `-interaction=batchmode -recorder -synctex=1
//! [-output-directory=DIR]`, the environment passed through
//! (`SOURCE_DATE_EPOCH` included), to the cross-run fixpoint (the files
//! the run wrote and reads back compared with the last run's, at most
//! [`RUNS`] runs), BibTeX and makeindex in process between runs and
//! `biber` from `PATH` when a `.bcf` asks for it. SyncTeX comes from
//! `lualatex` itself. `phitex watch` runs the whole build again when a
//! file changes; an editor's unsaved buffers are not built (they need a
//! saved file). No Lua VM is in phitex, nothing of this is in
//! phitex-flow; native builds only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::modern::{Options, Style, Target, ms};

/// The runs a build may take.
pub const RUNS: usize = 5;

/// The external engines, by the names a command line or a magic comment
/// may use, and the program each runs.
pub fn program_of(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "lualatex" => Some("lualatex"),
        "lualatex-dev" => Some("lualatex-dev"),
        "luatex" => Some("luatex"),
        "luahbtex" => Some("luahbtex"),
        "dvilualatex" => Some("dvilualatex"),
        _ => None,
    }
}

/// `name` on `PATH`, not phitex itself (phitex may be installed under
/// that name).
pub fn find_program(name: &str) -> Option<PathBuf> {
    let me = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| {
        p.is_file() && p.canonicalize().ok().as_ref() != me.as_ref()
    })
}

/// What an external build did.
pub struct Report {
    pub runs: Vec<(Duration, Option<i32>, String)>,
    /// The files that did not settle at the bound.
    pub unsettled: Vec<String>,
    pub notes: Vec<String>,
    /// The PDF written, if there is one.
    pub pdf: Option<PathBuf>,
    /// The errors in the last run's log (`! …` lines).
    pub errors: Vec<String>,
    pub wall: Duration,
}

/// The files the last run wrote and that a run reads back, from its
/// `.fls` (`OUTPUT` lines; not the log, the PDF, SyncTeX or the `.fls`).
fn written(dir: &Path, job: &str) -> Vec<PathBuf> {
    let fls = std::fs::read_to_string(dir.join(format!("{job}.fls"))).unwrap_or_default();
    let mut out: Vec<PathBuf> = Vec::new();
    for l in fls.lines() {
        let Some(f) = l.strip_prefix("OUTPUT ") else { continue };
        let p = PathBuf::from(f);
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let skip = [".log", ".pdf", ".dvi", ".fls", ".synctex.gz", ".synctex"].iter().any(|e| name.ends_with(e));
        if !skip && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn snapshot(files: &[PathBuf]) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    files.iter().map(|p| (p.clone(), std::fs::read(p).ok())).collect()
}

/// One external build of target `t` with `program`, to the fixpoint.
pub fn build(program: &Path, t: &Target, o: &Options) -> Report {
    let t0 = Instant::now();
    let job = t.job();
    let dir = t.output_dir.as_ref().map_or_else(|| PathBuf::from("."), PathBuf::from);
    let _ = std::fs::create_dir_all(&dir);
    let mut tools = crate::tools::Tools::default();
    let mut runs = Vec::new();
    let mut notes = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    let mut bcf_last: Option<Vec<u8>> = None;
    for k in 1..=RUNS {
        let before = snapshot(&written(&dir, &job));
        let mut cmd = Command::new(program);
        cmd.arg("-interaction=batchmode").arg("-recorder");
        if t.synctex {
            cmd.arg("-synctex=1");
        }
        if let Some(d) = &t.output_dir {
            cmd.arg(format!("-output-directory={d}"));
        }
        cmd.arg(&t.file).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
        let r0 = Instant::now();
        let status = cmd.status().ok().and_then(|s| s.code());
        let why = if k == 1 { "cold".to_owned() } else { format!("{} changed", changed.join(", ")) };
        runs.push((r0.elapsed(), status, why));
        if o.verbose > 0 {
            eprintln!("  run {k}: {} (exit {status:?})", ms(r0.elapsed()));
        }
        let files = written(&dir, &job);
        let after = snapshot(&files);
        changed = files
            .iter()
            .filter(|p| before.get(*p).cloned().flatten() != after.get(*p).cloned().flatten())
            .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            .collect();
        // (BibTeX and makeindex, in process, on the files as written)
        let writes: Vec<(String, String)> = files
            .iter()
            .filter_map(|p| {
                let n = p.strip_prefix(&dir).unwrap_or(p).to_string_lossy().into_owned();
                Some((n, crate::tex::chars(after.get(p)?.as_ref()?)))
            })
            .collect();
        let known = |n: &str| dir.join(n).is_file();
        for m in tools.after_run(&writes, &dir, &known) {
            if m.line.starts_with("biber") {
                continue;
            }
            notes.push(m.line);
            if let Some((n, b)) = m.side {
                let _ = std::fs::write(dir.join(n), b);
            }
            if let Some(text) = m.text {
                let p = dir.join(&m.file);
                let bytes = crate::tex::bytes(&text);
                if std::fs::read(&p).ok().as_deref() != Some(&bytes[..]) {
                    let _ = std::fs::write(&p, bytes);
                    changed.push(m.file);
                }
            }
        }
        // (biber, from PATH, when the .bcf asks for it and changed)
        let bcf = dir.join(format!("{job}.bcf"));
        if let Ok(b) = std::fs::read(&bcf)
            && bcf_last.as_ref() != Some(&b)
        {
            bcf_last = Some(b);
            match find_program("biber") {
                Some(biber) => {
                    let bbl = dir.join(format!("{job}.bbl"));
                    let old = std::fs::read(&bbl).ok();
                    let b0 = Instant::now();
                    let st = Command::new(biber)
                        .arg(&job)
                        .current_dir(&dir)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .status()
                        .ok()
                        .and_then(|s| s.code());
                    notes.push(format!("biber {job} in {}: exit {st:?}", ms(b0.elapsed())));
                    if std::fs::read(&bbl).ok() != old {
                        changed.push(format!("{job}.bbl"));
                    }
                }
                None => notes.push(format!("biber: {job}.bcf asks for it, and there is no biber on PATH")),
            }
        }
        if changed.is_empty() {
            break;
        }
    }
    let log = std::fs::read(dir.join(format!("{job}.log"))).unwrap_or_default();
    let errors = String::from_utf8_lossy(&log)
        .lines()
        .filter(|l| l.starts_with("! "))
        .map(|l| l[2..].to_owned())
        .collect();
    let pdf = dir.join(format!("{job}.pdf"));
    Report {
        unsettled: if runs.len() == RUNS { changed } else { Vec::new() },
        runs,
        notes,
        pdf: pdf.is_file().then_some(pdf),
        errors,
        wall: t0.elapsed(),
    }
}

/// The note every external build prints once.
pub fn banner(program: &str, path: &Path) -> String {
    format!(
        "{program} is the system's ({}): run as an external process, not incremental (every build runs \
         the whole document again)",
        path.display()
    )
}

/// What a build did, for the terminal; whether it succeeded.
pub fn report(r: &Report, o: &Options, st: &Style) -> bool {
    if !o.quiet {
        for n in &r.notes {
            eprintln!("{}", st.dim(&format!("  {n}")));
        }
    }
    for e in r.errors.iter().take(10) {
        eprintln!("{} {e}", st.red("!"));
    }
    if !r.unsettled.is_empty() {
        eprintln!(
            "{}: the written files did not settle in {} runs: {} still changed",
            st.yellow("warning"),
            r.runs.len(),
            r.unsettled.join(", ")
        );
    }
    let ok = r.errors.is_empty() && r.pdf.is_some() && r.runs.iter().all(|x| x.1 == Some(0));
    let what = r.pdf.as_ref().map_or_else(|| "no PDF".to_owned(), |p| p.display().to_string());
    let line = format!("{what}, in {} ({} run{}, external)", ms(r.wall), r.runs.len(), if r.runs.len() == 1 { "" } else { "s" });
    if ok {
        eprintln!("{}", st.green(&line));
    } else {
        eprintln!("{}", st.red(&line));
    }
    ok
}

/// `phitex build` with an external engine: the exit status.
pub fn main_build(program: &str, t: &Target, o: &Options, st: &Style) -> i32 {
    let Some(path) = find_program(program) else {
        eprintln!("{}: `{program}` is not on PATH (phitex runs the system's {program})", st.red("error"));
        return 2;
    };
    eprintln!("{}", st.dim(&format!("phitex: {}", banner(program, &path))));
    let r = build(&path, t, o);
    i32::from(!report(&r, o, st))
}

/// `phitex watch` with an external engine: the whole build again on each
/// change of a project file; never returns.
pub fn main_watch(program: &str, t: &Target, o: &Options, st: &Style) -> ! {
    let Some(path) = find_program(program) else {
        eprintln!("{}: `{program}` is not on PATH (phitex runs the system's {program})", st.red("error"));
        std::process::exit(2);
    };
    eprintln!("{}", st.dim(&format!("phitex: {}", banner(program, &path))));
    eprintln!(
        "{}",
        st.dim("phitex: an editor's unsaved buffers are not built with an external engine: save the file")
    );
    let view = if o.view.unwrap_or_else(|| std::io::IsTerminal::is_terminal(&std::io::stdout())) {
        match crate::view::View::start(None) {
            Ok(v) => {
                println!("phitex: the viewer is at {}", v.url());
                if o.open {
                    crate::view::open_browser(v.url());
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
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let outputs = |p: &Path| {
        let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        n.starts_with(&format!("{}.", t.job()))
            && !n.ends_with(".tex")
            && !n.ends_with(".bib")
    };
    let mut builds = 0usize;
    let mut rebuild = |why: &str| {
        builds += 1;
        if let Some(v) = &view {
            v.building(true);
        }
        let r = build(&path, t, o);
        let ok = report(&r, o, st);
        println!(
            "phitex: build {builds} ({why}) in {} ({} runs, external, not incremental){}",
            ms(r.wall),
            r.runs.len(),
            if ok { "" } else { "; errors" }
        );
        if let Some(v) = &view {
            let pdf = r.pdf.as_ref().and_then(|p| std::fs::read(p).ok()).map(|b| crate::session::shared(&b));
            v.built(&crate::view::Built {
                pdf,
                history: if ok { 0 } else { 2 },
                ms: r.wall.as_secs_f64() * 1e3,
                diagnostics: Vec::new(),
            });
            v.building(false);
        }
    };
    rebuild("cold");
    let mut last: std::collections::HashMap<_, _> = crate::watch::scan(&root).into_iter().filter(|(p, _)| !outputs(p)).collect();
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            if tx.send(Some(l)).is_err() {
                return;
            }
        }
        let _ = tx.send(None);
    });
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(None) => std::process::exit(0),
            Ok(Some(l)) => match l.trim() {
                "q" | "quit" => std::process::exit(0),
                "r" | "" => rebuild("asked"),
                other => eprintln!("phitex watch: `{other}`: r rebuilds, q quits"),
            },
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => std::process::exit(0),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now: std::collections::HashMap<_, _> =
            crate::watch::scan(&root).into_iter().filter(|(p, _)| !outputs(p)).collect();
        if now != last {
            let mut changed: Vec<String> = now
                .iter()
                .filter(|(p, s)| last.get(*p) != Some(s))
                .map(|(p, _)| p.strip_prefix(&root).unwrap_or(p).to_string_lossy().into_owned())
                .collect();
            changed.sort();
            rebuild(&changed.join(", "));
            last = crate::watch::scan(&root).into_iter().filter(|(p, _)| !outputs(p)).collect();
        }
    }
}
