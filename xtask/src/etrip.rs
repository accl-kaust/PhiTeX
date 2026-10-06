//! `cargo xtask etrip`: e-TeX's torture test, as web2c's
//! `etexdir/etriptest.test` runs it, on partex (as pdfTeX) and compared
//! with the installed `etex` (pdfTeX) run the same way, stored in
//! `refs/pdftex/etrip/` (`--oracle` makes them; they are also made when
//! missing).
//!
//! Three phases: Knuth's trip test in e-TeX's compatibility mode (`c`),
//! again in extended mode (`x`), then the e-TeX test proper (`e`). Masked
//! as `cargo xtask trip` masks.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::trip::{Masker, diff_summary, print_diff};

/// Files compared, in the order the test produces them.
const FILES: &[&str] = &[
    "ctripin.fot",
    "ctripin.log",
    "ctrip.fot",
    "ctrip.log",
    "ctripos.tex",
    "ctrip.typ",
    "xtripin.fot",
    "xtripin.log",
    "xtrip.fot",
    "xtrip.log",
    "xtripos.tex",
    "xtrip.typ",
    "etripin.fot",
    "etripin.log",
    "etrip.fot",
    "etrip.log",
    "etrip.out",
    "etrip.typ",
];

/// pdfTeX's string pool outgrows `etrip/texmf.cnf`'s (the pool is masked).
const POOL_SIZE: &str = "200000";

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    let machine = args.iter().any(|a| a == "--machine");
    let refs = root.join("refs/pdftex/etrip");
    if args.iter().any(|a| a == "--oracle") || !refs.join("etrip.log").exists() {
        let work = root.join("target/etrip-oracle");
        let etex = |w: &Path| {
            let mut c = Command::new("etex");
            c.current_dir(w);
            c
        };
        run_etrip(root, &work, &etex)?;
        if refs.exists() {
            fs::remove_dir_all(&refs)?;
        }
        fs::create_dir_all(&refs)?;
        for f in FILES {
            if work.join(f).exists() {
                fs::copy(work.join(f), refs.join(f))?;
            }
        }
        println!("oracle outputs stored in refs/pdftex/etrip");
    }

    let status = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["build", "--release", "-p", "partex-cli"])
        .status()?;
    ensure!(status.success(), "building phitex failed");
    let partex = root.join("target/release/phitex");
    let work = root.join(if machine {
        "target/etrip-machine"
    } else {
        "target/etrip"
    });
    let engine = |w: &Path| {
        let mut c = Command::new(&partex);
        c.current_dir(w).arg("--compat=pdftex");
        if machine {
            c.envs(crate::trip::MACHINE_ENV.iter().copied());
        }
        c
    };
    run_etrip(root, &work, &engine)?;

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
        "\netrip{}: {}/{} files identical (outputs in {})",
        if machine { " (machine mode)" } else { "" },
        FILES.len() - failed,
        FILES.len(),
        work.display()
    );
    ensure!(failed == 0, "etrip test differs");
    Ok(())
}

/// Run the three phases in `work` with the engine `tex` makes.
fn run_etrip(root: &Path, work: &Path, tex: &dyn Fn(&Path) -> Command) -> Result<()> {
    let web2c = root.join("upstream/texlive-source/texk/web2c");
    let trip = web2c.join("triptrap");
    let etrip = web2c.join("etexdir/etrip");
    if work.exists() {
        fs::remove_dir_all(work)?;
    }
    fs::create_dir_all(work)?;
    let tool = |prog: &str| {
        let mut c = Command::new(prog);
        c.current_dir(work);
        c
    };
    let run = |mut c: Command, stdin: Option<&Path>, stdout: &str| -> Result<()> {
        c.env("TEXMFCNF", &etrip)
            .env("pool_size", POOL_SIZE)
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .stdin(match stdin {
                Some(f) => Stdio::from(fs::File::open(f)?),
                None => Stdio::null(),
            })
            .stdout(fs::File::create(work.join(stdout))?)
            .stderr(Stdio::null());
        c.status()?; // errors by design
        Ok(())
    };
    let mv = |from: &str, to: &str| -> Result<()> {
        if work.join(from).exists() {
            fs::rename(work.join(from), work.join(to))?;
        }
        Ok(())
    };
    let dvitype = |dvi: &str, out: &str| -> Result<()> {
        if !work.join(dvi).exists() {
            return Ok(());
        }
        let mut c = tool("dvitype");
        c.args([
            "-output-level=2",
            "-dpi=72.27",
            "-page-start=*.*.*.*.*.*.*.*.*.*",
            dvi,
        ]);
        run(c, None, out)
    };
    let ini = |w: &Path| {
        let mut c = tex(w);
        c.args(["--progname=einitex", "--ini"]);
        c
    };
    let vir = |w: &Path| {
        let mut c = tex(w);
        c.arg("--progname=etex");
        c
    };

    let mut c = tool("pltotf");
    c.arg(trip.join("trip.pl")).arg("trip.tfm");
    run(c, None, "pltotf.out")?;
    fs::copy(trip.join("trip.tex"), work.join("trip.tex"))?;
    for (mode, first, second) in [
        ("c", trip.join("trip1.in"), trip.join("trip2.in")),
        ("x", etrip.join("etrip1.in"), etrip.join("trip2.in")),
    ] {
        run(ini(work), Some(&first), &format!("{mode}tripin.fot"))?;
        mv("trip.log", &format!("{mode}tripin.log"))?;
        if work.join("trip.fmt").exists() {
            run(vir(work), Some(&second), &format!("{mode}trip.fot"))?;
            mv("trip.log", &format!("{mode}trip.log"))?;
            mv("tripos.tex", &format!("{mode}tripos.tex"))?;
            dvitype("trip.dvi", &format!("{mode}trip.typ"))?;
        }
        for f in ["trip.fmt", "trip.dvi"] {
            let _ = fs::remove_file(work.join(f));
        }
    }

    let mut c = tool("pltotf");
    c.arg(etrip.join("etrip.pl")).arg("etrip.tfm");
    run(c, None, "pltotf.out")?;
    fs::copy(etrip.join("etrip.tex"), work.join("etrip.tex"))?;
    run(ini(work), Some(&etrip.join("etrip2.in")), "etripin.fot")?;
    mv("etrip.log", "etripin.log")?;
    if work.join("etrip.fmt").exists() {
        run(vir(work), Some(&etrip.join("etrip3.in")), "etrip.fot")?;
        dvitype("etrip.dvi", "etrip.typ")?;
    }
    Ok(())
}
