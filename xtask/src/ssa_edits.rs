//! `cargo xtask ssa-edits`: SSA mode's edit sequences against plain partex
//! (DESIGN.md 4.3, item 8). The harness is `scripts/ssa-edits`, run on the
//! release binary; the options pass through to it (`--case NAME`,
//! `--fixpoint`, `--check`, `--env K=V`, ...). Like every run of the
//! binary, it must be inside the sandbox (`scripts/sandbox cargo xtask
//! ssa-edits`).

use std::path::Path;
use std::process::Command;

use anyhow::{Result, ensure};

/// Build the release binary, then run the harness with `args`.
pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let status = Command::new(env!("CARGO"))
        .current_dir(root)
        .args(["build", "--release", "-p", "partex-cli"])
        .status()?;
    ensure!(status.success(), "building partex failed");
    harness(root, args)
}

/// The harness with `args`, on the release binary already built.
pub fn harness(root: &Path, args: &[String]) -> Result<()> {
    let status = Command::new("python3")
        .arg(root.join("scripts/ssa-edits"))
        .args(args)
        .current_dir(root)
        .status()?;
    ensure!(
        status.success(),
        "SSA mode's edit sequences differ from plain partex (or did not run)"
    );
    Ok(())
}
