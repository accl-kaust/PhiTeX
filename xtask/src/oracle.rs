//! Run the installed reference engines on the corpus and store their raw outputs.
//!
//! Layout of `refs/` (not committed):
//! - `refs/<engine>/<test id without extension>/` — every file the run produced
//!   (raw `.log`, l3build-normalized `.<engine>.log`, `.pdf`/`.dvi`, `.aux`, …).
//! - `refs/_runs/…` — l3build's console output per (module, config, engine).
//! - `refs/index.json` — status per test and engine, plus tool versions.
//!
//! l3build tests run through l3build itself, so the oracle sees exactly the
//! commands, format builds and normalization upstream uses. Each worker slot
//! gets a private copy of the upstream LaTeX trees because l3build writes its
//! build directory next to the sources.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Engines whose binaries are the oracle. `etex` is pdfTeX in DVI mode (TeX Live).
const ENGINES: &[&str] = &["tex", "etex", "pdftex", "xetex", "luatex"];

/// Tests per l3build invocation.
const CHUNK: usize = 40;

const USAGE: &str = "\
usage: scripts/sandbox cargo xtask oracle [options]

options:
  --engine <e>     only this engine (repeatable; default: tex etex pdftex xetex luatex)
  --filter <re>    only tests whose id matches the regex
  --jobs <n>       parallel workers (default: available CPUs)
  --list           print the jobs without running them
  --partex         run phitex in place of the engines (as `pdftex`, `etex`,
                   `pdflatex`, …, first in PATH), into target/partex-refs";

#[derive(Deserialize)]
struct Manifest {
    upstream: BTreeMap<String, String>,
    tests: Vec<Test>,
}

#[derive(Deserialize)]
struct Test {
    id: String,
    kind: String,
    input: String,
    engines: Vec<String>,
    l3build: Option<L3Run>,
}

#[derive(Deserialize)]
struct L3Run {
    module: String,
    config: String,
    testdir: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Index {
    meta: BTreeMap<String, String>,
    /// test id -> engine -> outcome
    results: BTreeMap<String, BTreeMap<String, Outcome>>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Outcome {
    status: Status,
    /// Files stored under `refs/<engine>/<test>/`.
    files: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "kebab-case")]
enum Status {
    /// The oracle ran and its output matches the upstream expectation.
    MatchesUpstream,
    /// The oracle ran; its output differs from the upstream expectation
    /// (expected: upstream files track the dev kernel / another build).
    DiffersFromUpstream,
    /// The oracle ran; there is no automatic upstream comparison (trip: the
    /// upstream files have documented acceptable differences).
    Ran,
    /// The oracle ran but produced no log.
    NoOutput,
    /// No installed binary can serve as the oracle.
    NoOracle,
    /// Not run by this harness yet.
    NotRun,
}

enum Job {
    Trip,
    L3 {
        /// Repository under `upstream/` (`latex2e`, `latex3`).
        repo: String,
        /// Paths relative to the repository.
        module: String,
        testdir: String,
        config: String,
        engine: String,
        /// (test id, file stem, testfiledir relative to the repository)
        tests: Vec<(String, String, String)>,
    },
}

impl Job {
    fn label(&self) -> String {
        match self {
            Job::Trip => "trip (tex)".to_owned(),
            Job::L3 {
                repo,
                module,
                config,
                engine,
                tests,
                ..
            } => format!("{repo}/{module} [{config}] {engine}: {} tests", tests.len()),
        }
    }

    fn weight(&self) -> usize {
        match self {
            Job::Trip => 1,
            Job::L3 { tests, .. } => tests.len(),
        }
    }
}

struct Options {
    engines: BTreeSet<String>,
    filter: Option<Regex>,
    jobs: usize,
    list: bool,
    /// Run partex (release build) in place of the engines.
    partex: bool,
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let opts = parse_args(args)?;
    ensure!(
        std::env::var_os("PARTEX_SANDBOX").is_some() || opts.list,
        "the oracle runs upstream code; run it inside the sandbox:\n  scripts/sandbox cargo xtask oracle"
    );

    let manifest: Manifest = serde_json::from_str(
        &fs::read_to_string(root.join("corpus/manifest.json"))
            .context("corpus/manifest.json missing; run `cargo xtask corpus`")?,
    )?;
    let tests: Vec<&Test> = manifest
        .tests
        .iter()
        .filter(|t| opts.filter.as_ref().is_none_or(|re| re.is_match(&t.id)))
        .collect();

    let mut index = Index::default();
    let (mut jobs, skipped) = plan(&tests, &opts.engines, &mut index);
    jobs.sort_by_key(|j| std::cmp::Reverse(j.weight()));
    let total: usize = jobs.iter().map(Job::weight).sum();
    println!(
        "{} jobs, {total} test runs; {skipped} tests have no runnable oracle",
        jobs.len()
    );
    if opts.list {
        for j in &jobs {
            println!("  {}", j.label());
        }
        return Ok(());
    }

    let (shim, refs, slots) = locations(root, opts.partex)?;
    fs::create_dir_all(&refs)?;

    // Largest first; workers pop from the end, so store the queue reversed.
    let mut queue: Vec<(usize, Job)> = jobs.into_iter().enumerate().collect();
    queue.sort_by_key(|(_, j)| j.weight());
    let n_jobs = queue.len();
    let queue = Mutex::new(queue);
    let results = Mutex::new(index.results);
    let errors = Mutex::new(Vec::new());
    let started = Instant::now();

    std::thread::scope(|s| {
        for slot in 0..opts.jobs {
            let (queue, results, errors) = (&queue, &results, &errors);
            let slot_dir = slots.join(slot.to_string());
            let (root, refs, shim) = (root, &refs, shim.as_deref());
            s.spawn(move || {
                loop {
                    let Some((i, job)) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let t0 = Instant::now();
                    match run_job(root, refs, &slot_dir, &job, shim) {
                        Ok(out) => {
                            let summary = summarize(out.values());
                            println!(
                                "[{:>3}/{n_jobs}] {} ({:.0}s) {summary}",
                                i + 1,
                                job.label(),
                                t0.elapsed().as_secs_f64()
                            );
                            let engine = match &job {
                                Job::Trip => "tex",
                                Job::L3 { engine, .. } => engine,
                            };
                            let mut r = results.lock().unwrap();
                            for (id, o) in out {
                                r.entry(id).or_default().insert(engine.to_owned(), o);
                            }
                        }
                        Err(e) => {
                            println!("[{:>3}/{n_jobs}] {} FAILED: {e:#}", i + 1, job.label());
                            errors
                                .lock()
                                .unwrap()
                                .push(format!("{}: {e:#}", job.label()));
                        }
                    }
                }
            });
        }
    });

    let index_path = refs.join("index.json");
    let merged = write_index(
        &index_path,
        results.into_inner().unwrap(),
        &manifest.upstream,
    )?;
    let all = merged.results.values().flat_map(BTreeMap::values);
    println!(
        "done in {:.0}s; index: {} ({})",
        started.elapsed().as_secs_f64(),
        index_path
            .strip_prefix(root)
            .unwrap_or(&index_path)
            .display(),
        summarize(all)
    );
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        bail!("{} jobs failed:\n  {}", errors.len(), errors.join("\n  "));
    }
    Ok(())
}

/// Merge into an existing index so filtered runs refresh only what they ran.
fn write_index(
    path: &Path,
    results: BTreeMap<String, BTreeMap<String, Outcome>>,
    upstream: &BTreeMap<String, String>,
) -> Result<Index> {
    let mut merged: Index = fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    for (id, per_engine) in results {
        merged.results.entry(id).or_default().extend(per_engine);
    }
    merged.meta = meta(upstream);
    fs::write(path, serde_json::to_string_pretty(&merged)? + "\n")?;
    Ok(merged)
}

fn parse_args(args: &[String]) -> Result<Options> {
    let mut opts = Options {
        engines: BTreeSet::new(),
        filter: None,
        jobs: std::thread::available_parallelism().map_or(4, std::num::NonZero::get),
        list: false,
        partex: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || {
            it.next()
                .with_context(|| format!("{a} needs a value\n\n{USAGE}"))
        };
        match a.as_str() {
            "--engine" => {
                let e = value()?;
                ensure!(ENGINES.contains(&e.as_str()), "unsupported engine `{e}`");
                opts.engines.insert(e.clone());
            }
            "--filter" => opts.filter = Some(Regex::new(value()?)?),
            "--jobs" => opts.jobs = value()?.parse::<usize>()?.max(1),
            "--list" => opts.list = true,
            "--partex" => opts.partex = true,
            "-h" | "--help" => bail!("{USAGE}"),
            _ => bail!("unknown option `{a}`\n\n{USAGE}"),
        }
    }
    if opts.engines.is_empty() {
        opts.engines = ENGINES.iter().map(|e| (*e).to_owned()).collect();
    }
    Ok(opts)
}

/// Group tests into jobs; record tests without a runnable oracle in `index`.
fn plan(tests: &[&Test], engines: &BTreeSet<String>, index: &mut Index) -> (Vec<Job>, usize) {
    let mut jobs = Vec::new();
    let mut skipped = 0;
    let mut groups: BTreeMap<(String, String, String, String, String), Vec<_>> = BTreeMap::new();
    let mut mark = |id: &str, engine: &str, status| {
        index.results.entry(id.to_owned()).or_default().insert(
            engine.to_owned(),
            Outcome {
                status,
                files: Vec::new(),
            },
        );
    };

    for t in tests {
        match (t.kind.as_str(), &t.l3build) {
            ("trip", _) if t.id == "trip/trip" => {
                if engines.contains("tex") {
                    jobs.push(Job::Trip);
                }
            }
            // The installed `etex` is pdfTeX; there is no standalone e-TeX binary.
            ("trip", _) => {
                mark(&t.id, "etex", Status::NoOracle);
                skipped += 1;
            }
            // web2c `*.test` scripts expect a build tree; not wired up yet.
            ("shell-test", _) => {
                for e in &t.engines {
                    mark(&t.id, e, Status::NotRun);
                }
                skipped += 1;
            }
            ("lvt" | "pvt", Some(l3)) => {
                let (repo, module) = split_repo(&l3.module);
                let (_, testdir) = split_repo(&l3.testdir);
                let (_, input) = split_repo(&t.input);
                let path = Path::new(&input);
                let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
                let testfiledir = path.parent().unwrap().to_string_lossy().into_owned();
                let mut any = false;
                for e in t.engines.iter().filter(|e| engines.contains(*e)) {
                    any = true;
                    groups
                        .entry((
                            repo.clone(),
                            module.clone(),
                            testdir.clone(),
                            l3.config.clone(),
                            e.clone(),
                        ))
                        .or_default()
                        .push((t.id.clone(), stem.clone(), testfiledir.clone()));
                }
                if !any {
                    skipped += 1;
                }
            }
            _ => skipped += 1,
        }
    }
    for ((repo, module, testdir, config, engine), tests) in groups {
        // Chunks run in parallel (each slot has its own tree); each pays one format build.
        for chunk in tests.chunks(CHUNK) {
            jobs.push(Job::L3 {
                repo: repo.clone(),
                module: module.clone(),
                testdir: testdir.clone(),
                config: config.clone(),
                engine: engine.clone(),
                tests: chunk.to_vec(),
            });
        }
    }
    (jobs, skipped)
}

/// `upstream/latex2e/base` -> (`latex2e`, `base`).
fn split_repo(path: &str) -> (String, String) {
    let rest = path.strip_prefix("upstream/").unwrap_or(path);
    let (repo, sub) = rest.split_once('/').unwrap_or((rest, ""));
    (repo.to_owned(), sub.to_owned())
}

/// Where a run's results and work go, and for partex, the directory of
/// links to it under the engines' names (first in `PATH`).
fn locations(root: &Path, partex: bool) -> Result<(Option<PathBuf>, PathBuf, PathBuf)> {
    Ok(if partex {
        (
            Some(partex_shim(root)?),
            root.join("target/partex-refs"),
            root.join("target/partex-oracle/slots"),
        )
    } else {
        (None, root.join("refs"), root.join("target/oracle/slots"))
    })
}

/// `target/partex-shim`: links to the release build of phitex named as the
/// programs l3build runs.
fn partex_shim(root: &Path) -> Result<PathBuf> {
    let status = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["build", "--release", "-p", "partex-cli"])
        .status()?;
    ensure!(status.success(), "building phitex failed");
    let shim = root.join("target/partex-shim");
    fs::create_dir_all(&shim)?;
    for name in ["tex", "etex", "pdftex", "latex", "pdflatex", "initex"] {
        let link = shim.join(name);
        if link.symlink_metadata().is_ok() {
            fs::remove_file(&link)?;
        }
        std::os::unix::fs::symlink(root.join("target/release/phitex"), &link)?;
    }
    // partex's own formats, as TeX Live's fmtutil.cnf makes them (the
    // installed ones are pdfTeX's)
    let formats = shim.join("formats");
    fs::create_dir_all(&formats)?;
    for (name, engine, ini) in [
        ("tex", "tex", "tex.ini"),
        ("etex", "pdftex", "*etex.ini"),
        ("pdftex", "pdftex", "*pdfetex.ini"),
        ("latex", "pdftex", "*latex.ini"),
        ("pdflatex", "pdftex", "*pdflatex.ini"),
    ] {
        let mut cmd = Command::new(shim.join(engine));
        cmd.current_dir(&formats).args([
            "-ini",
            "-interaction=nonstopmode",
            &format!("-jobname={name}"),
        ]);
        if engine == "pdftex" {
            cmd.arg("-translate-file=cp227.tcx");
        }
        let out = cmd.arg(ini).stdin(Stdio::null()).output()?;
        ensure!(
            formats.join(format!("{name}.fmt")).exists(),
            "building partex's {name}.fmt failed:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    Ok(shim)
}

fn run_job(
    root: &Path,
    refs: &Path,
    slot: &Path,
    job: &Job,
    shim: Option<&Path>,
) -> Result<BTreeMap<String, Outcome>> {
    match job {
        Job::Trip => run_trip(root, refs, &slot.join("trip")),
        Job::L3 {
            repo,
            module,
            testdir,
            config,
            engine,
            tests,
        } => {
            let tree = slot_tree(root, slot, repo)?;
            let testdir = tree.join(testdir);
            if testdir.exists() {
                fs::remove_dir_all(&testdir)?;
            }
            let mut cmd = Command::new("l3build");
            if let Some(shim) = shim {
                let path = std::env::var_os("PATH").unwrap_or_default();
                let mut dirs = vec![shim.to_path_buf()];
                dirs.extend(std::env::split_paths(&path));
                cmd.env("PATH", std::env::join_paths(dirs)?);
                // the current directory first (l3build builds formats
                // there), and an empty entry: the default path after it
                let mut formats = std::ffi::OsString::from(".:");
                formats.push(shim.join("formats"));
                formats.push(":");
                cmd.env("TEXFORMATS", formats);
            }
            cmd.current_dir(tree.join(module))
                .args(["check", "-e", engine]);
            if config != "build" {
                cmd.args(["-c", config]);
            }
            cmd.args(tests.iter().map(|(_, stem, _)| stem));
            let out = cmd
                .stdin(Stdio::null())
                .output()
                .with_context(|| format!("running l3build in {repo}/{module}"))?;
            // l3build exits non-zero when any test differs from its .tlg; that is data.
            let log_dir = refs.join("_runs").join(repo).join(module).join(config);
            fs::create_dir_all(&log_dir)?;
            let mut console = out.stdout;
            console.extend_from_slice(&out.stderr);
            fs::write(log_dir.join(format!("{engine}.txt")), &console)?;

            let mut results = BTreeMap::new();
            for (id, stem, testfiledir) in tests {
                let inputs = fs::read_dir(tree.join(testfiledir))?
                    .filter_map(|e| e.ok().map(|e| e.file_name()))
                    .collect::<BTreeSet<_>>();
                let dest = test_ref_dir(refs, engine, id);
                let files = harvest(&testdir, stem, &inputs, &dest)?;
                let status = if !files.iter().any(|f| f == &format!("{stem}.log")) {
                    Status::NoOutput
                } else if testdir.join(format!("{stem}.{engine}.diff")).exists() {
                    Status::DiffersFromUpstream
                } else {
                    Status::MatchesUpstream
                };
                results.insert(id.clone(), Outcome { status, files });
            }
            Ok(results)
        }
    }
}

/// A private copy of `upstream/<repo>` for this slot (sources only, no `.git`/`build`).
fn slot_tree(root: &Path, slot: &Path, repo: &str) -> Result<PathBuf> {
    let tree = slot.join(repo);
    if !tree.exists() {
        fs::create_dir_all(slot)?;
        let tmp = slot.join(format!("{repo}.partial"));
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        let status = Command::new("cp")
            .args(["-a", "--reflink=auto"])
            .arg(root.join("upstream").join(repo))
            .arg(&tmp)
            .status()?;
        ensure!(status.success(), "copying upstream/{repo} failed");
        for d in [".git", "build"] {
            if tmp.join(d).exists() {
                fs::remove_dir_all(tmp.join(d))?;
            }
        }
        fs::rename(&tmp, &tree)?;
    }
    Ok(tree)
}

/// Copy `<stem>.*` outputs from `dir` to `dest`, skipping files copied in from the
/// test file directory (the `.lvt` itself, upstream `.tlg`s, support files).
fn harvest(
    dir: &Path,
    stem: &str,
    inputs: &BTreeSet<std::ffi::OsString>,
    dest: &Path,
) -> Result<Vec<String>> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    let mut files = Vec::new();
    if !dir.is_dir() {
        return Ok(files);
    }
    let prefix = format!("{stem}.");
    for e in fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name();
        let s = name.to_string_lossy();
        if !s.starts_with(&prefix) || inputs.contains(&name) || !e.file_type()?.is_file() {
            continue;
        }
        fs::create_dir_all(dest)?;
        fs::copy(e.path(), dest.join(&name))?;
        files.push(s.into_owned());
    }
    files.sort();
    Ok(files)
}

fn test_ref_dir(refs: &Path, engine: &str, id: &str) -> PathBuf {
    let id = Path::new(id);
    refs.join(engine).join(id.with_extension(""))
}

/// Knuth's trip test, following web2c's `triptest.test`: an INITEX pass that
/// dumps `trip.fmt`, then the main run, then `dvitype` on the result.
///
/// Note: `tex` ignores `SOURCE_DATE_EPOCH`, so the banner/date lines of the
/// logs carry the wall-clock time; comparisons must mask them.
fn run_trip(root: &Path, refs: &Path, work: &Path) -> Result<BTreeMap<String, Outcome>> {
    let src = root.join("upstream/texlive-source/texk/web2c/triptrap");
    if work.exists() {
        fs::remove_dir_all(work)?;
    }
    fs::create_dir_all(work)?;
    fs::copy(src.join("trip.tex"), work.join("trip.tex"))?;
    let tool = |prog: &str| {
        let mut c = Command::new(prog);
        c.current_dir(work)
            .env("TEXMFCNF", &src)
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C");
        c
    };
    let run = |mut c: Command, stdin: Option<&str>, stdout: &str| -> Result<()> {
        c.stdin(match stdin {
            Some(f) => Stdio::from(fs::File::open(src.join(f))?),
            None => Stdio::null(),
        });
        c.stdout(fs::File::create(work.join(stdout))?);
        c.status()?; // TeX exits non-zero here by design (trip has errors).
        Ok(())
    };

    let mut c = tool("pltotf");
    c.arg(src.join("trip.pl")).arg("trip.tfm");
    run(c, None, "pltotf.out")?;
    let mut c = tool("tftopl");
    c.args(["trip.tfm", "trip.pl"]);
    run(c, None, "tftopl.out")?;

    let mut c = tool("tex");
    c.args(["--progname=initex", "--ini"]);
    run(c, Some("trip1.in"), "tripin.fot")?;
    ensure!(
        work.join("trip.fmt").exists(),
        "trip.fmt not created by trip1.in"
    );
    fs::rename(work.join("trip.log"), work.join("tripin.log"))?;

    let mut c = tool("tex");
    c.arg("--progname=tex");
    run(c, Some("trip2.in"), "trip.fot")?;

    let mut c = tool("dvitype");
    c.args([
        "-output-level=2",
        "-dpi=72.27",
        "-page-start=*.*.*.*.*.*.*.*.*.*",
        "trip.dvi",
    ]);
    run(c, None, "trip.typ")?;

    let dest = test_ref_dir(refs, "tex", "trip/trip");
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::create_dir_all(&dest)?;
    let mut files = Vec::new();
    for e in fs::read_dir(work)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "trip.tex" || !e.file_type()?.is_file() {
            continue;
        }
        fs::copy(e.path(), dest.join(&name))?;
        files.push(name);
    }
    files.sort();
    let status = if files.iter().any(|f| f == "trip.log") {
        Status::Ran
    } else {
        Status::NoOutput
    };
    Ok(BTreeMap::from([(
        "trip/trip".to_owned(),
        Outcome { status, files },
    )]))
}

fn summarize<'a>(outcomes: impl Iterator<Item = &'a Outcome>) -> String {
    let mut counts: BTreeMap<Status, usize> = BTreeMap::new();
    for o in outcomes {
        *counts.entry(o.status).or_default() += 1;
    }
    counts
        .iter()
        .map(|(s, n)| {
            let s = serde_json::to_value(s).unwrap();
            format!("{} {n}", s.as_str().unwrap_or("?"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Versions of every tool whose behaviour the refs depend on.
fn meta(upstream: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for (tool, arg) in [
        ("tex", "--version"),
        ("pdftex", "--version"),
        ("xetex", "--version"),
        ("luatex", "--version"),
        ("l3build", "--version"),
        ("dvitype", "--version"),
    ] {
        let v = Command::new(tool)
            .arg(arg)
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    // l3build's first line is a banner; its release line has digits.
                    .find(|l| l.chars().any(|c| c.is_ascii_digit()))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "unavailable".to_owned());
        m.insert(format!("version/{tool}"), v);
    }
    for (repo, commit) in upstream {
        m.insert(format!("upstream/{repo}"), commit.clone());
    }
    m
}
