//! Developer tasks: `cargo xtask <command>`.

mod bench;
mod corpus;
mod e2e;
mod etrip;
mod mask;
mod oracle;
mod origins;
mod parallel;
mod sections;
mod ssa_edits;
mod trip;

use std::path::PathBuf;

use anyhow::{Result, bail};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  corpus    index test suites under upstream/ into corpus/manifest.json
  oracle    run the installed engines on the corpus, store outputs in refs/
  sections  tex.web port coverage (sections cited as `§N` under crates/)
  e2e       compare partex with the oracle engines on end-to-end jobs
  ssa-edits the incremental cases as rebuilds of one SSA process, every stage
            against plain partex (scripts/ssa-edits; its options pass through)
  parallel  how much of an SSA build could run at once (PARTEX_SSA_DAG dumps)
  origins   check DIR/JOB.origins.jsonl against DIR/JOB.pdf and the sources
            (`--glyphs`: with glyphs.tex's expectations; `--dump`: each glyph)
  trip      run Knuth's trip test on partex and compare with refs/tex/trip
  etrip     run e-TeX's etrip test on partex and compare with pdfTeX's run
  bench     time partex against pdflatex, record JSON in bench/results/ (--pgf: PGF subset)
  check     fmt, clippy (with and without tracing), wasm32 build of core, tests";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("corpus") => corpus::run(&workspace_root()),
        Some("check") => check(),
        Some("bench") => bench::run(&workspace_root(), &args[1..]),
        Some("sections") => sections::run(&workspace_root(), &args[1..]),
        Some("oracle") => oracle::run(&workspace_root(), &args[1..]),
        Some("e2e") => e2e::run(&workspace_root(), &args[1..]),
        Some("ssa-edits") => ssa_edits::run(&workspace_root(), &args[1..]),
        Some("parallel") => parallel::run(&args[1..]),
        Some("origins") => origins::run(&args[1..]),
        Some("trip") => trip::run(&workspace_root(), &args[1..]),
        Some("etrip") => etrip::run(&workspace_root(), &args[1..]),
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

/// Core crates that must build for `wasm32-unknown-unknown`.
const WASM_CRATES: &[&str] = &["partex-core", "partex-incr", "partex-ssa", "partex-trace"];

fn check() -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let run = |args: &[&str], target: Option<&str>| -> Result<()> {
        println!("$ cargo {}", args.join(" "));
        let mut cmd = std::process::Command::new(&cargo);
        cmd.args(args);
        if let Some(dir) = target {
            cmd.env("CARGO_TARGET_DIR", workspace_root().join(dir));
        }
        let status = cmd.status()?;
        if !status.success() {
            bail!("`cargo {}` failed", args.join(" "));
        }
        Ok(())
    };
    run(&["fmt", "--all", "--check"], None)?;
    // Two tracks at once: the lints and unit tests (in their own target
    // directory, so they do not wait on the release build's lock), and
    // the release build with the oracle comparisons, which run side by
    // side.
    let mut wasm = vec!["check", "--target", "wasm32-unknown-unknown"];
    for c in WASM_CRATES {
        wasm.extend(["-p", c]);
    }
    let lint_steps: Vec<Vec<&str>> = vec![
        vec![
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        vec![
            "clippy",
            "--workspace",
            "--all-targets",
            "--features",
            "partex-cli/trace",
            "--",
            "-D",
            "warnings",
        ],
        wasm,
        vec!["test", "--workspace", "--quiet"],
    ];
    let root = workspace_root();
    std::thread::scope(|scope| -> Result<()> {
        let lints = scope.spawn(|| -> Result<()> {
            for args in &lint_steps {
                run(args, Some("target/lint"))?;
            }
            Ok(())
        });
        run(&["build", "--release", "-p", "partex-cli"], None)?;
        // Knuth's trip test against the stored reference outputs, e-TeX's
        // torture test against pdfTeX's, and the e2e cases (in parallel
        // themselves).
        let trip = scope.spawn(|| trip::run(&root, &[]));
        let etrip = scope.spawn(|| etrip::run(&root, &[]));
        // (and both in machine mode, the `partex watch` default)
        let knuth_machine = scope.spawn(|| trip::run(&root, &[String::from("--machine")]));
        let etex_machine = scope.spawn(|| etrip::run(&root, &[String::from("--machine")]));
        // (SSA mode's edit sequences, every stage against plain partex:
        // DESIGN.md 4.3, item 8)
        let ssa_edits = scope.spawn(|| ssa_edits_both(&root));
        let e2e = e2e::run(&root, &[]);
        // (and in machine mode, the default of `partex watch`: after the
        // plain run, which the format caches are shared with, in a
        // process of its own with the engine's environment)
        let e2e_machine = (|| -> Result<()> {
            let status = std::process::Command::new(std::env::current_exe()?)
                .arg("e2e")
                .env("PARTEX_MACHINE", "1")
                .env("XTASK_E2E_DIR", "target/e2e-machine")
                .status()?;
            if !status.success() {
                bail!("the e2e cases differ in machine mode");
            }
            Ok(())
        })();
        let join = |h: std::thread::ScopedJoinHandle<'_, Result<()>>, what: &str| {
            h.join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("{what} panicked")))
        };
        let results = [
            ("trip", join(trip, "trip")),
            ("etrip", join(etrip, "etrip")),
            (
                "trip (machine mode)",
                join(knuth_machine, "trip (machine mode)"),
            ),
            (
                "etrip (machine mode)",
                join(etex_machine, "etrip (machine mode)"),
            ),
            ("e2e", e2e),
            ("e2e (machine mode)", e2e_machine),
            ("ssa-edits", join(ssa_edits, "ssa-edits")),
            ("lints", join(lints, "lints")),
        ];
        let failed: Vec<String> = results
            .into_iter()
            .filter_map(|(what, r)| r.err().map(|e| format!("{what}: {e}")))
            .collect();
        if !failed.is_empty() {
            bail!("{}", failed.join("; "));
        }
        Ok(())
    })
}

/// The SSA edit sequences in both trip modes: a rebuild settles the .aux
/// loop in trips (DESIGN 3.7), so the oracle runs to its fixed point; with
/// `PARTEX_SSA_TRIPS=1`, one trip a rebuild, against one plain pass a stage.
fn ssa_edits_both(root: &std::path::Path) -> Result<()> {
    let brief = String::from("--brief");
    ssa_edits::harness(root, &[brief.clone(), String::from("--fixpoint")])?;
    ssa_edits::harness(
        root,
        &[
            brief,
            String::from("--env"),
            String::from("PARTEX_SSA_TRIPS=1"),
            String::from("--work"),
            String::from("target/ssa-edits-trips1"),
        ],
    )
}
