//! `partex outline FILE.tex`: the document's structure read from its
//! source without running TeX (`phitex-doc`, DESIGN 4.3.6): the outline,
//! labels, references, citations, the file graph and the guards, with the
//! time it took. `--edits` times the layer's update after edits.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use phitex_doc::compare::{compare_labels, compare_toc};
use phitex_doc::{Files, Project, Timing, Views};

const USAGE: &str = "\
usage: partex outline FILE.tex [--json] [--aux FILE.aux] [--toc FILE.toc]
                      [--edits EDITS.txt [--only NAME]] [--repeat N] [--check]

The document's outline, labels, references, citations, files and guards,
read from its source without running TeX.

  --json          the report as JSON
  --aux FILE      compare the labels with an .aux file's \\newlabel lines
  --toc FILE      compare the table of contents with a .toc file
  --edits FILE    apply each edit of FILE (`name<TAB>path|from|to`, `;;`
                  between edits typed one after another, `<NL>` a newline),
                  time the layer's update, and undo it
  --only NAME     only that edit
  --repeat N      open (and edit) N times: times are the minimum and median
  --check         after each edit, compare the facts with a fresh reading";

/// The file system, from the main file's directory.
struct Fs {
    root: PathBuf,
}

impl Files for Fs {
    fn read(&self, path: &str) -> Option<String> {
        let bytes = std::fs::read(self.root.join(path)).ok()?;
        Some(match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        })
    }

    fn exists(&self, path: &str) -> bool {
        self.root.join(path).is_file()
    }
}

/// The files a first reading read, in memory: later readings time the
/// layer, not the disk.
struct Cached {
    texts: HashMap<String, String>,
    missing: HashSet<String>,
    under: Rc<dyn Files>,
}

impl Files for Cached {
    fn read(&self, path: &str) -> Option<String> {
        if self.missing.contains(path) {
            return None;
        }
        self.texts
            .get(path)
            .cloned()
            .or_else(|| self.under.read(path))
    }

    fn exists(&self, path: &str) -> bool {
        self.texts.contains_key(path) || (!self.missing.contains(path) && self.under.exists(path))
    }
}

struct Options {
    file: String,
    json: bool,
    aux: Option<String>,
    toc: Option<String>,
    edits: Option<String>,
    only: Option<String>,
    repeat: usize,
    check: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        file: String::new(),
        json: false,
        aux: None,
        toc: None,
        edits: None,
        only: None,
        repeat: 1,
        check: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--json" => o.json = true,
            "--aux" => o.aux = Some(value()?),
            "--toc" => o.toc = Some(value()?),
            "--edits" => o.edits = Some(value()?),
            "--only" => o.only = Some(value()?),
            "--repeat" => {
                o.repeat = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--repeat takes a positive number")?;
            }
            "--check" => o.check = true,
            "-h" | "--help" => return Err(String::new()),
            f if !f.starts_with('-') && o.file.is_empty() => f.clone_into(&mut o.file),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    if o.file.is_empty() {
        return Err(String::new());
    }
    Ok(o)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// The minimum and the median.
fn min_median(v: &mut [f64]) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

/// The machine's load average (1 minute), for the times' conditions.
fn load() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "?".to_owned())
}

/// Run `partex outline` with `args` (after `outline`).
pub fn main(args: &[String]) -> ! {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            if e.is_empty() {
                println!("{USAGE}");
                std::process::exit(if args.is_empty() { 2 } else { 0 });
            }
            eprintln!("partex outline: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let path = Path::new(&o.file);
    if !path.is_file() {
        eprintln!("partex outline: {}: no such file", o.file);
        std::process::exit(2);
    }
    let root = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let fs: Rc<dyn Files> = Rc::new(Fs { root });

    // (the first reading, from the disk; then from memory)
    let t = Instant::now();
    let first = Project::open(&name, fs.clone());
    let cold = ms(t);
    let cached: Rc<dyn Files> = Rc::new(Cached {
        texts: first
            .files()
            .map(|(id, f)| (f.path.clone(), first.text(id)))
            .collect(),
        missing: first.missing().map(str::to_owned).collect(),
        under: fs,
    });
    let mut opens = Vec::with_capacity(o.repeat);
    let mut project = first;
    for _ in 0..o.repeat {
        let t = Instant::now();
        project = Project::open(&name, cached.clone());
        opens.push(ms(t));
    }
    let mut folds = Vec::with_capacity(o.repeat);
    for _ in 0..o.repeat {
        let t = Instant::now();
        let v = project.views();
        folds.push(ms(t));
        drop(v);
    }
    let v = project.views();
    let (open_min, open_med) = min_median(&mut opens);
    let (fold_min, fold_med) = min_median(&mut folds);
    let bytes: usize = project.files().map(|(_, f)| f.bytes()).sum();
    let timing = Timing {
        phases: vec![
            ("first reading, from disk".to_owned(), cold),
            ("parse and facts (min)".to_owned(), open_min),
            ("parse and facts (median)".to_owned(), open_med),
            ("views (min)".to_owned(), fold_min),
            ("views (median)".to_owned(), fold_med),
        ],
    };
    if o.json {
        print!("{}", phitex_doc::json(&project, &v, Some(&timing)));
    } else {
        print!("{}", phitex_doc::report(&project, &v, Some(&timing)));
        println!(
            "        ({} bytes in {} files, {} runs, load {})",
            bytes,
            project.files().count(),
            o.repeat,
            load()
        );
    }
    if let Some(aux) = &o.aux {
        compare_aux_file(&project, &v, aux);
    }
    if let Some(toc) = &o.toc {
        compare_toc_file(&project, &v, toc);
    }
    drop(v);
    if let Some(edits) = &o.edits {
        run_edits(&mut project, edits, &o);
    }
    std::process::exit(0);
}

fn compare_aux_file(p: &Project, v: &Views<'_>, aux: &str) {
    let Ok(text) = std::fs::read_to_string(aux) else {
        eprintln!("partex outline: cannot read {aux}");
        return;
    };
    let c = compare_labels(v, &text);
    println!(
        "\nLabels against {aux}: {} in both, {} cleveref twins (key@cref) not compared, {} only in the .aux, {} only in the source",
        c.matched,
        c.cleveref,
        c.only_aux.len(),
        c.only_source.len()
    );
    for k in &c.only_aux {
        // (where the source has the key: the package that wrote it)
        let mut seen = false;
        for (id, f) in p.files() {
            for (n, line) in p.text(id).lines().enumerate() {
                if line.contains(k.as_str()) {
                    println!("  .aux only: {k}  ({}:{}: {})", f.path, n + 1, line.trim());
                    seen = true;
                }
            }
        }
        if !seen {
            println!("  .aux only: {k}  (not in the source)");
        }
    }
    for (k, l) in &c.only_source {
        println!("  source only: {k} at {}:{}", p.path(l.file), l.line);
    }
}

fn compare_toc_file(p: &Project, v: &Views<'_>, toc: &str) {
    let Ok(text) = std::fs::read_to_string(toc) else {
        eprintln!("partex outline: cannot read {toc}");
        return;
    };
    let c = compare_toc(p, v, &text);
    println!(
        "\nTable of contents against {toc}: {} entries the same, {} differ",
        c.matched,
        c.diffs.len()
    );
    let show = |e: &Option<phitex_doc::compare::Entry>| match e {
        Some(e) => format!(
            "{} {} {}{}",
            e.level,
            e.number.as_deref().unwrap_or("-"),
            e.title,
            e.loc
                .map(|l| format!(" ({}:{})", p.path(l.file), l.line))
                .unwrap_or_default()
        ),
        None => "(none)".to_owned(),
    };
    for (ours, theirs) in &c.diffs {
        println!("  source: {}\n  .toc:   {}", show(ours), show(theirs));
    }
}

/// An edit: a file, the text replaced, and its replacement.
struct Edit {
    path: String,
    from: String,
    to: String,
}

fn parse_edits(text: &str) -> Vec<(String, Vec<Edit>)> {
    let nl = |s: &str| s.replace("<NL>", "\n");
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (name, specs) = l.split_once('\t')?;
            let edits = specs
                .split(";;")
                .filter_map(|s| {
                    let mut parts = s.splitn(3, '|');
                    Some(Edit {
                        path: parts.next()?.to_owned(),
                        from: nl(parts.next()?),
                        to: nl(parts.next()?),
                    })
                })
                .collect();
            Some((name.to_owned(), edits))
        })
        .collect()
}

/// Counts the edits change.
fn counts(p: &Project) -> [usize; 5] {
    let v = p.views();
    [
        v.outline.len(),
        v.labels.len(),
        v.refs.len(),
        v.unresolved().count(),
        v.files.len(),
    ]
}

fn run_edits(p: &mut Project, file: &str, o: &Options) {
    let Ok(text) = std::fs::read_to_string(file) else {
        eprintln!("partex outline: cannot read {file}");
        return;
    };
    println!(
        "\nEdits ({} runs each: minimum / median, load {}):",
        o.repeat,
        load()
    );
    for (name, edits) in parse_edits(&text) {
        if o.only.as_ref().is_some_and(|n| *n != name) {
            continue;
        }
        let before = counts(p);
        let (mut apply, mut views, mut undo) = (Vec::new(), Vec::new(), Vec::new());
        let mut read = 0;
        let mut ok = true;
        for run in 0..o.repeat {
            // (each edit applied where its text is; the last one timed)
            let mut done = Vec::new();
            let mut last = 0.0;
            for e in &edits {
                let Some(id) = p.file_id(&e.path) else {
                    println!("  {name}: {} is not loaded", e.path);
                    ok = false;
                    break;
                };
                let Some(at) = p.text(id).find(&e.from) else {
                    println!("  {name}: `{}` not found in {}", e.from, e.path);
                    ok = false;
                    break;
                };
                let t = Instant::now();
                let u = p.edit(id, at, e.from.len(), &e.to);
                last = ms(t);
                read = u.read;
                done.push((id, at, e));
            }
            if !ok {
                break;
            }
            apply.push(last);
            let t = Instant::now();
            let v = p.views();
            views.push(ms(t));
            drop(v);
            if run == 0 {
                let after = counts(p);
                let names = ["headings", "labels", "references", "unresolved", "files"];
                let delta: Vec<String> = names
                    .iter()
                    .zip(before.iter().zip(after.iter()))
                    .filter(|(_, (b, a))| b != a)
                    .map(|(n, (b, a))| format!("{n} {b} -> {a}"))
                    .collect();
                if !delta.is_empty() {
                    println!("  {name}: {}", delta.join(", "));
                }
                if o.check
                    && let Err(e) = p.check()
                {
                    println!("  {name}: CHECK FAILED: {e}");
                }
            }
            let t = Instant::now();
            for (id, at, e) in done.iter().rev() {
                p.edit(*id, *at, e.to.len(), &e.from);
            }
            undo.push(ms(t));
        }
        if !ok || apply.is_empty() {
            continue;
        }
        let (a0, a1) = min_median(&mut apply);
        let (v0, v1) = min_median(&mut views);
        let (u0, u1) = min_median(&mut undo);
        println!(
            "  {name:<11} update {a0:.4} / {a1:.4} ms ({read} paragraphs read)   views {v0:.3} / {v1:.3} ms   undo {u0:.4} / {u1:.4} ms"
        );
        if o.check
            && let Err(e) = p.check()
        {
            println!("  {name}: CHECK FAILED after undo: {e}");
        }
    }
}
