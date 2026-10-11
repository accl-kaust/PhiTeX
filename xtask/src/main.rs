//! Developer tasks: `cargo xtask <command>`.

mod corpus;
mod sections;

use std::path::PathBuf;

use anyhow::{Result, bail};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  corpus    index test suites under upstream/ into corpus/manifest.json
  sections  tex.web coverage (sections cited as `§N` under crates/)
  check     clippy, the wasm32 build of the pure crates, the tests";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("corpus") => corpus::run(&workspace_root()),
        Some("check") => check(),
        Some("sections") => sections::run(&workspace_root(), &args[1..]),
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

/// Pure crates that must build for `wasm32-unknown-unknown`.
const WASM_CRATES: &[&str] = &[
    "phitex-codecs",
    "phitex-defuse",
    "phitex-otf",
    "phitex-source",
    "phitex-typeset",
    "phitex-xdvipdfmx",
];

fn check() -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let run = |args: &[&str]| -> Result<()> {
        println!("$ cargo {}", args.join(" "));
        let status = std::process::Command::new(&cargo).args(args).status()?;
        if !status.success() {
            bail!("`cargo {}` failed", args.join(" "));
        }
        Ok(())
    };
    run(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])?;
    let mut wasm = vec!["check", "--target", "wasm32-unknown-unknown"];
    for c in WASM_CRATES {
        wasm.extend(["-p", c]);
    }
    run(&wasm)?;
    run(&["test", "--workspace", "--release", "--quiet"])
}
