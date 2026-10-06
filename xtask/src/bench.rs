//! `cargo xtask bench`: time partex against the reference `pdflatex` on
//! the same machine and record the figures as JSON in `bench/results/`
//! (DESIGN.md §8), one file per commit (`<commit>.local.json`, not
//! committed, for a tree with changes).
//!
//! Cases: `bench/inputs/article.tex` (2 pages: amsmath, `TikZ`, hyperref):
//! a plain run on settled auxiliary files against `pdflatex`'s, then
//! `-converge` cold, unchanged from its saved session, and after an edit
//! of the body. With `--pgf`, `bench/inputs/pgfsub.tex` (four chapters of
//! the PGF manual, about 115 pages, in `upstream/pgf`): a plain run
//! against `pdflatex`'s, and a cold `-converge`.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

/// One measured quantity over some runs.
#[derive(Serialize)]
struct Figure {
    case: &'static str,
    /// Wall-clock seconds of each run.
    seconds: Vec<f64>,
    /// Peak resident memory of each run, KiB.
    max_rss_kib: Vec<u64>,
    median_seconds: f64,
    min_seconds: f64,
    /// Whether the PDF is byte-identical to `pdflatex`'s from the same
    /// inputs (plain runs only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pdf_identical: Option<bool>,
}

#[derive(Serialize)]
struct Report {
    commit: String,
    dirty: bool,
    date: String,
    host: String,
    cpus: usize,
    reference: String,
    figures: Vec<Figure>,
}

/// Where and how a case runs.
struct Job<'a> {
    /// The directory the engine runs in.
    dir: &'a Path,
    /// Its output directory, if not `dir`.
    out: Option<&'a Path>,
    /// The input file, as given to the engine.
    input: &'a str,
    formats: &'a Path,
    cache: &'a Path,
}

impl Job<'_> {
    fn out_dir(&self) -> &Path {
        self.out.unwrap_or(self.dir)
    }

    /// Run `program` with `args` then the job's options and input; the
    /// wall-clock seconds and peak memory.
    fn time(&self, program: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<(f64, u64)> {
        let stats = self.cache.with_extension("time");
        let mut cmd = Command::new("/usr/bin/time");
        cmd.arg("-o")
            .arg(&stats)
            .args(["-f", "%e %M"])
            .arg(program)
            .args(args)
            .arg("-interaction=batchmode");
        if let Some(o) = self.out {
            cmd.arg(format!("-output-directory={}", o.display()));
        }
        // (partex's formats for partex only: pdflatex keeps its own)
        if program.ends_with("phitex") {
            cmd.env("TEXFORMATS", format!("{}:", self.formats.display()));
        }
        cmd.arg(self.input)
            .current_dir(self.dir)
            .env("TEXINPUTS", format!("{}:.:", self.out_dir().display()))
            .env("SOURCE_DATE_EPOCH", "1700000000")
            .env("FORCE_SOURCE_DATE", "1")
            .env("TZ", "UTC")
            .env("PARTEX_CACHE_DIR", self.cache)
            .env_remove("PARTEX_PERSIST")
            .envs(env.iter().copied())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // (TeX's exit status says whether the document had errors; the
        // benchmark documents have none, and the PDF check tells)
        cmd.status()
            .with_context(|| format!("running {}", program.display()))?;
        let text = fs::read_to_string(&stats)?;
        // (the last line: before it, time says if the status was not 0)
        let mut w = text.lines().last().unwrap_or_default().split_whitespace();
        let secs = w.next().and_then(|s| s.parse().ok());
        let kib = w.next().and_then(|s| s.parse().ok());
        match (secs, kib) {
            (Some(s), Some(k)) => Ok((s, k)),
            _ => bail!("unexpected /usr/bin/time output: {text}"),
        }
    }
}

fn figure(case: &'static str, runs: &[(f64, u64)], pdf_identical: Option<bool>) -> Figure {
    let seconds: Vec<f64> = runs.iter().map(|r| r.0).collect();
    let max_rss_kib = runs.iter().map(|r| r.1).collect();
    let mut sorted = seconds.clone();
    sorted.sort_by(f64::total_cmp);
    let median_seconds = sorted[sorted.len() / 2];
    let min_seconds = sorted[0];
    Figure {
        case,
        seconds,
        max_rss_kib,
        median_seconds,
        min_seconds,
        pdf_identical,
    }
}

fn repeat(n: usize, mut f: impl FnMut() -> Result<(f64, u64)>) -> Result<Vec<(f64, u64)>> {
    (0..n).map(|_| f()).collect()
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(root).output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Remove the files a run of `job` writes in `dir`.
fn clean(dir: &Path, job: &str) {
    for ext in ["aux", "toc", "out", "pdf", "log"] {
        let _ = fs::remove_file(dir.join(format!("{job}.{ext}")));
    }
}

/// The engines and the format directory.
struct Tools {
    partex: PathBuf,
    pdflatex: PathBuf,
    formats: PathBuf,
}

/// The 2-page article's figures.
fn article(root: &Path, t: &Tools, work: &Path) -> Result<Vec<Figure>> {
    let (partex, pdflatex, formats) = (&t.partex, &t.pdflatex, &t.formats);
    let engine = ["--compat=pdftex", "-fmt=pdflatex"];
    let converge = [&engine[..], &["-converge"]].concat();
    let fmt = ["-fmt=pdflatex"];
    let mut figures = Vec::new();
    let (p, o) = (work.join("article/p"), work.join("article/o"));
    for d in [&p, &o] {
        fs::create_dir_all(d)?;
        fs::copy(root.join("bench/inputs/article.tex"), d.join("article.tex"))?;
    }
    let cache = work.join("article/cache");
    let job = |dir| Job {
        dir,
        out: None,
        input: "article.tex",
        formats,
        cache: &cache,
    };
    // settle both, then plain runs
    for _ in 0..3 {
        job(&o).time(pdflatex, &fmt, &[])?;
    }
    job(&p).time(
        partex,
        &[&engine[..], &["-converge"]].concat(),
        &[("PARTEX_PERSIST", "0")],
    )?;
    let reference = repeat(10, || job(&o).time(pdflatex, &fmt, &[]))?;
    let plain = repeat(10, || job(&p).time(partex, &engine, &[]))?;
    let same = fs::read(p.join("article.pdf"))? == fs::read(o.join("article.pdf"))?;
    figures.push(figure("article: pdflatex, plain run", &reference, None));
    figures.push(figure("article: plain run", &plain, Some(same)));
    let cold = repeat(3, || {
        clean(&p, "article");
        let _ = fs::remove_dir_all(&cache);
        job(&p).time(partex, &converge, &[])
    })?;
    figures.push(figure("article: -converge, cold", &cold, None));
    let unchanged = repeat(5, || job(&p).time(partex, &converge, &[]))?;
    figures.push(figure("article: -converge, unchanged", &unchanged, None));
    let tex = p.join("article.tex");
    let original = fs::read_to_string(&tex)?;
    let edited = original.replace(
        "\\end{document}",
        "Another sentence in the body.\n\\end{document}",
    );
    let edit = repeat(3, || {
        fs::write(&tex, &original)?;
        job(&p).time(partex, &converge, &[])?;
        fs::write(&tex, &edited)?;
        job(&p).time(partex, &converge, &[])
    })?;
    fs::write(&tex, &original)?;
    figures.push(figure("article: -converge, body edit", &edit, None));

    Ok(figures)
}

/// The PGF subset's figures.
fn pgfsub(root: &Path, t: &Tools, work: &Path) -> Result<Vec<Figure>> {
    let (partex, pdflatex, formats) = (&t.partex, &t.pdflatex, &t.formats);
    let engine = ["--compat=pdftex", "-fmt=pdflatex"];
    let converge = [&engine[..], &["-converge"]].concat();
    let fmt = ["-fmt=pdflatex"];
    let mut figures = Vec::new();
    let src = root.join("upstream/pgf/doc/generic/pgf");
    ensure!(src.exists(), "fetch upstream sources first");
    let (p, o) = (work.join("pgfsub/p"), work.join("pgfsub/o"));
    let input = root.join("bench/inputs/pgfsub.tex");
    let input = input.to_str().context("path")?;
    let cache = work.join("pgfsub/cache");
    let job = |out| Job {
        dir: &src,
        out: Some(out),
        input,
        formats,
        cache: &cache,
    };
    for d in [&p, &o] {
        fs::create_dir_all(d)?;
    }
    for _ in 0..3 {
        job(&o).time(pdflatex, &fmt, &[])?;
    }
    for e in fs::read_dir(&o)? {
        let path = e?.path();
        if path
            .extension()
            .is_some_and(|x| x == "aux" || x == "toc" || x == "out")
        {
            fs::copy(&path, p.join(path.file_name().context("name")?))?;
        }
    }
    let reference = repeat(1, || job(&o).time(pdflatex, &fmt, &[]))?;
    let plain = repeat(1, || job(&p).time(partex, &engine, &[]))?;
    // (the PDF's /ID covers its path, which differs: compare without)
    let strip = |b: Vec<u8>| -> Vec<u8> {
        let s = String::from_utf8_lossy(&b).into_owned();
        regex::Regex::new(r"/ID \[<[0-9A-F]+> <[0-9A-F]+>\]")
            .expect("regex")
            .replace_all(&s, "")
            .into_owned()
            .into_bytes()
    };
    let same = strip(fs::read(p.join("pgfsub.pdf"))?) == strip(fs::read(o.join("pgfsub.pdf"))?);
    figures.push(figure("pgfsub: pdflatex, plain run", &reference, None));
    figures.push(figure("pgfsub: plain run", &plain, Some(same)));
    let cold = repeat(1, || {
        for e in fs::read_dir(&p)? {
            fs::remove_file(e?.path())?;
        }
        let _ = fs::remove_dir_all(&cache);
        job(&p).time(partex, &converge, &[])
    })?;
    figures.push(figure("pgfsub: -converge, cold", &cold, None));
    Ok(figures)
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let pgf = args.iter().any(|a| a == "--pgf");
    let t = Tools {
        partex: root.join("target/release/phitex"),
        pdflatex: PathBuf::from("pdflatex"),
        formats: root.join("target/partex-shim/formats"),
    };
    ensure!(
        t.partex.exists() && t.formats.join("pdflatex.fmt").exists(),
        "build target/release/phitex and its pdflatex format first \
         (scripts/build-pdflatex-format.sh)"
    );
    let work = root.join("target/bench");
    let _ = fs::remove_dir_all(&work);
    let mut figures = article(root, &t, &work)?;
    if pgf {
        figures.extend(pgfsub(root, &t, &work)?);
    }
    let commit = git(root, &["rev-parse", "--short=12", "HEAD"])?;
    let dirty = !git(root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    let report = Report {
        commit: commit.clone(),
        dirty,
        date: git(root, &["log", "-1", "--format=%cI"])?,
        host: fs::read_to_string("/proc/sys/kernel/hostname")
            .unwrap_or_default()
            .trim()
            .to_owned(),
        cpus: std::thread::available_parallelism().map_or(1, usize::from),
        reference: String::from_utf8_lossy(
            &Command::new("pdflatex").arg("--version").output()?.stdout,
        )
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned(),
        figures,
    };
    let dir = root.join("bench/results");
    fs::create_dir_all(&dir)?;
    let name = if dirty {
        format!("{commit}.local.json")
    } else {
        format!("{commit}.json")
    };
    fs::write(
        dir.join(&name),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    let mut table = String::new();
    for f in &report.figures {
        let rss = f.max_rss_kib.iter().max().copied().unwrap_or(0);
        let _ = write!(
            table,
            "{:<34} {:>8.3} s median {:>8.3} s min {:>7} MiB",
            f.case,
            f.median_seconds,
            f.min_seconds,
            rss / 1024
        );
        if let Some(same) = f.pdf_identical {
            table.push_str(if same {
                "  PDF identical"
            } else {
                "  PDF DIFFERS"
            });
        }
        table.push('\n');
    }
    print!("{table}");
    println!("bench: wrote bench/results/{name}");
    Ok(())
}
