//! `cargo xtask trip`: run Knuth's trip test on partex, the way
//! `oracle::run_trip` runs it on `tex`, and compare with the stored
//! reference outputs in `refs/tex/trip/trip/`.
//!
//! Dates, the distribution's version string and TeX's memory statistics
//! (see `mask.rs`) are masked; everything else must be byte-identical.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use regex::Regex;

/// Files compared, in the order trip produces them.
const FILES: &[&str] = &[
    "tripin.fot",
    "tripin.log",
    "trip.fot",
    "trip.log",
    "8terminal.tex",
    "tripos.tex",
    "trip.typ",
];

/// `--machine`: the engine runs in machine mode (`PARTEX_MACHINE=1`), cut
/// into small regions and checked (every region replayed), in its own
/// work directory.
pub const MACHINE_ENV: &[(&str, &str)] = &[
    ("PARTEX_MACHINE", "1"),
    ("PARTEX_MACHINE_GRAIN", "50"),
    ("PARTEX_MACHINE_SANITIZE", "1"),
];

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    let machine = args.iter().any(|a| a == "--machine");
    let status = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["build", "--release", "-p", "partex-cli"])
        .status()?;
    ensure!(status.success(), "building phitex failed");
    let partex = root.join("target/release/phitex");
    let src = root.join("upstream/texlive-source/texk/web2c/triptrap");
    let refs = root.join("refs/tex/trip/trip");
    let work = root.join(if machine {
        "target/trip-machine"
    } else {
        "target/trip"
    });
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    fs::create_dir_all(&work)?;
    fs::copy(src.join("trip.tex"), work.join("trip.tex"))?;
    fs::copy(refs.join("trip.tfm"), work.join("trip.tfm"))
        .context("refs/tex/trip/trip/trip.tfm (run `cargo xtask oracle trip` first)")?;

    let run = |args: &[&str], stdin: &str, stdout: &str| -> Result<()> {
        Command::new(&partex)
            .arg("--compat=tex")
            .envs(MACHINE_ENV.iter().copied().filter(|_| machine))
            .current_dir(&work)
            .env("TEXMFCNF", &src)
            .args(args)
            .stdin(Stdio::from(fs::File::open(src.join(stdin))?))
            .stdout(fs::File::create(work.join(stdout))?)
            .status()?; // trip has errors by design
        Ok(())
    };
    run(&["--progname=initex", "--ini"], "trip1.in", "tripin.fot")?;
    if work.join("trip.log").exists() {
        fs::rename(work.join("trip.log"), work.join("tripin.log"))?;
    }
    if work.join("trip.fmt").exists() {
        run(&["--progname=tex"], "trip2.in", "trip.fot")?;
    }
    if work.join("trip.dvi").exists() {
        // `dvitype` only reads our output here, as a comparison tool.
        let out = Command::new("dvitype")
            .current_dir(&work)
            .args([
                "-output-level=2",
                "-dpi=72.27",
                "-page-start=*.*.*.*.*.*.*.*.*.*",
                "trip.dvi",
            ])
            .stdin(Stdio::null())
            .output()?;
        fs::write(work.join("trip.typ"), out.stdout)?;
    }

    let mask = Masker::new()?;
    let mut failed = 0;
    for f in FILES {
        let want = fs::read(refs.join(f)).with_context(|| format!("reference {f}"))?;
        let Ok(got) = fs::read(work.join(f)) else {
            println!("MISSING  {f}");
            failed += 1;
            continue;
        };
        let (want, got) = (mask.apply(&want), mask.apply(&got));
        if want == got {
            println!("ok       {f}");
        } else {
            failed += 1;
            let (n, first) = diff_summary(&want, &got);
            println!("DIFFERS  {f}: {n} lines differ, first at line {first}");
            if verbose {
                print_diff(&want, &got);
            }
        }
    }
    println!(
        "\ntrip{}: {}/{} files identical (outputs in {})",
        if machine { " (machine mode)" } else { "" },
        FILES.len() - failed,
        FILES.len(),
        work.display()
    );
    ensure!(failed == 0, "trip test differs");
    Ok(())
}

/// Masks run-dependent text: dates, times and the version string.
pub struct Masker(Vec<(Regex, &'static str)>);

impl Masker {
    pub fn new() -> Result<Self> {
        let mut v = vec![
            (Regex::new(r"\(TeX Live \d+[^)]*\)")?, "(TeX Live)"),
            (Regex::new(r"\d{4}\.\d{1,2}\.\d{1,2}(:\d{4})?")?, "DATE"),
            (Regex::new(r" \d{1,2} [A-Z]{3} \d{4} \d{2}:\d{2}")?, " DATE"),
        ];
        v.extend(crate::mask::memory_statistics());
        Ok(Self(v))
    }
    pub fn apply(&self, b: &[u8]) -> String {
        let mut s = String::from_utf8_lossy(b).into_owned();
        for (re, rep) in &self.0 {
            s = re.replace_all(&s, *rep).into_owned();
        }
        s
    }
}

pub fn diff_summary(a: &str, b: &str) -> (usize, usize) {
    let (a, b): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
    let n = a.len().max(b.len());
    let differing: Vec<usize> = (0..n).filter(|&i| a.get(i) != b.get(i)).collect();
    (differing.len(), differing.first().map_or(0, |i| i + 1))
}

/// A small line diff around the first difference.
pub fn print_diff(a: &str, b: &str) {
    let (a, b): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
    let Some(first) = (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i)) else {
        return;
    };
    let from = first.saturating_sub(3);
    for i in from..(first + 12).min(a.len().max(b.len())) {
        match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) if x == y => println!("   {:5} {x}", i + 1),
            (x, y) => {
                if let Some(x) = x {
                    println!(" - {:5} {x}", i + 1);
                }
                if let Some(y) = y {
                    println!(" + {:5} {y}", i + 1);
                }
            }
        }
    }
}
