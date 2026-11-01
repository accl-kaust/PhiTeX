//! The recorder: runs pdfTeX or pdfLaTeX on a document with e-TeX tracing
//! on, in a scratch directory, and parses the log into a trace.
//!
//! The settings are injected through a wrapper file the engine reads
//! first:
//!
//! - `pdflatex`: the wrapper sets the tracing parameters and `\input`s
//!   the document (LaTeX allows `\input` before `\documentclass`: the
//!   document file is read as if its text stood in the wrapper), with
//!   `-jobname` set to the document's name so `\jobname`-derived files
//!   keep their names. The format (`pdflatex.fmt`) is not traced; the
//!   trace starts at the document's first line (the preamble included).
//! - `pdftex` (plain): `pdftex -ini -etex` on a wrapper that sets
//!   `\pdfoutput=1`, `\input plain`, then the tracing parameters, then
//!   `\input`s the document: plain.tex is loaded untraced, like a format.
//!
//! `max_print_line` is raised so that the log's lines are never wrapped
//! (the parser relies on it); `TEXINPUTS` adds the document's directory
//! so that files beside it (`\input`s, `.bbl`, graphics) are found.

use std::fs;
use std::io::{self, BufReader, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::parse::{self, Options};
use crate::step::{Meta, Record};

/// The tracing parameters, as the wrapper sets them.
pub const SETTINGS: &str = "\\tracingonline=0 \\tracingcommands=3 \\tracingmacros=2 \\tracingassigns=1 \\tracinggroups=1 \\tracingifs=1 \\tracingrestores=1 \\tracingnesting=2 \\tracingscantokens=1 \\tracinglostchars=0 ";

/// Which engine runs the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Pdflatex,
    Pdftex,
}

impl Engine {
    /// Parses `pdflatex` / `pdftex`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "pdflatex" => Ok(Self::Pdflatex),
            "pdftex" => Ok(Self::Pdftex),
            _ => Err(format!("unknown engine {s}: pdflatex or pdftex")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Pdflatex => "pdflatex",
            Self::Pdftex => "pdftex",
        }
    }
}

/// A recording's result.
#[derive(Debug)]
pub struct Recorded {
    /// The log parsed.
    pub log: PathBuf,
    /// Steps written.
    pub steps: u64,
    /// Log lines the parser didn't recognise.
    pub unrecognised: u64,
    /// The engine's exit status (errors in the document don't stop the
    /// recording: batch mode continues).
    pub status: Option<i32>,
}

/// Runs `main` through `engine` in `scratch` and writes the trace to
/// `out`.
pub fn record(
    main: &Path,
    engine: Engine,
    scratch: &Path,
    out: &Path,
    opts: Options,
) -> io::Result<Recorded> {
    let main = main.canonicalize()?;
    let stem = main
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| io::Error::other("the document needs a UTF-8 name"))?
        .to_owned();
    let dir = main
        .parent()
        .ok_or_else(|| io::Error::other("the document needs a directory"))?;
    fs::create_dir_all(scratch)?;
    let wrapper = scratch.join("phitex-trace-wrapper.tex");
    let main_s = main
        .to_str()
        .ok_or_else(|| io::Error::other("a UTF-8 path"))?;
    let text = match engine {
        Engine::Pdflatex => format!("{SETTINGS}\n\\input{{{main_s}}}\n"),
        Engine::Pdftex => {
            format!("\\pdfoutput=1 \\input plain\n{SETTINGS}\n\\input {main_s} \n\\end\n")
        }
    };
    fs::write(&wrapper, text)?;
    let mut cmd = Command::new(engine.name());
    if engine == Engine::Pdftex {
        cmd.args(["-ini", "-etex"]);
    }
    cmd.args([
        "-interaction=batchmode",
        "-jobname",
        &stem,
        "phitex-trace-wrapper.tex",
    ])
    .current_dir(scratch)
    .env("max_print_line", "1000000")
    .env("error_line", "254")
    .env("half_error_line", "238")
    .env("TEXINPUTS", format!("{}:", dir.display()))
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    let status = cmd.status()?;
    let log = scratch.join(format!("{stem}.log"));
    let main_name = format!(
        "./{}",
        main.file_name().and_then(|s| s.to_str()).unwrap_or(&stem)
    );
    let (steps, unrecognised) = parse_log_file(&log, &main_name, engine.name(), out, opts)?;
    Ok(Recorded {
        log,
        steps,
        unrecognised,
        status: status.code(),
    })
}

/// Parses an existing log of `main` into the trace `out`: steps written
/// and log lines not recognised.
pub fn parse_log_file(
    log: &Path,
    main: &str,
    engine: &str,
    out: &Path,
    opts: Options,
) -> io::Result<(u64, u64)> {
    let reader = BufReader::with_capacity(1 << 20, fs::File::open(log)?);
    let mut writer = BufWriter::with_capacity(1 << 20, fs::File::create(out)?);
    let meta = Record::Meta {
        meta: Meta {
            main: main.to_owned(),
            engine: engine.to_owned(),
            conservative: opts.conservative,
        },
    };
    serde_json::to_writer(&mut writer, &meta)?;
    writer.write_all(b"\n")?;
    let mut steps = 0;
    let unrecognised = parse::parse_log(reader, opts, |step| {
        steps += 1;
        serde_json::to_writer(&mut writer, &step)?;
        writer.write_all(b"\n")
    })?;
    writer.flush()?;
    Ok((steps, unrecognised))
}

/// Fixes the main file's name as the log prints it: the wrapper `\input`s
/// it by absolute path, so the log shows `(/abs/path/main.tex`; the
/// analyzer matches a step's file by its file name.
#[must_use]
pub fn same_file(log_name: &str, main: &str) -> bool {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_owned();
    base(log_name) == base(main)
}
