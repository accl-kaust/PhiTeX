//! The modern command line: `phitex build`, `phitex watch`, … (clap: the
//! options anywhere on the line, a coloured sectioned `--help`, typos
//! suggested). What to build comes from the command line, `phitex.toml`
//! and the main file's magic comments (`config.rs`); the build is a
//! a document session (`session.rs`), to the cross-run fixpoint,
//! BibTeX and makeindex included; the terminal is `ui.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::builder::styling::{AnsiColor, Styles};
use clap::{Args, ColorChoice, Parser, Subcommand};

use crate::config;
use crate::session::{self, Job, Report, Session, Status};
use crate::tex::{self, Output};
pub use crate::ui::{Style, Ui, ms};

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().bold())
    .usage(AnsiColor::Green.on_default().bold())
    .literal(AnsiColor::Cyan.on_default().bold())
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().bold())
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Yellow.on_default());

const ABOUT: &str = "\
PhiTeX: TeX documents as graph reduction

A document is built to its fixpoint (BibTeX and makeindex included) and,
under `watch`, kept up to date as its files and the editor's buffers
change, the pages live in the browser.";

/// The sections after the options (styled as clap's headers; the escapes
/// are stripped when colour is off).
fn after_help() -> String {
    let h = STYLES.get_header();
    let l = STYLES.get_literal();
    format!(
        "{h}Examples:{h:#}
  {l}phitex build paper.tex{l:#}              the PDF beside it, pdflatex for LaTeX
  {l}phitex watch -o out paper.tex{l:#}       rebuild on each change, the viewer in the browser
  {l}phitex -j 4 check paper.tex{l:#}         build on four threads, write nothing
  {l}phitex sync paper.tex:120{l:#}           the watch's viewer shows that line
  {l}phitex clean --all{l:#}                  remove the cache of formats

{h}Configuration:{h:#}
  Without a file, `phitex.toml` (here or above) names it:
      main = \"paper.tex\"   engine = \"pdflatex\"   output-dir = \"out\"
      copy-pdf = true   (or a directory, relative to phitex.toml's)
  A `% !TEX program = …` or `% !TEX root = …` comment in the file is honoured.

{h}TeX's own command line:{h:#}
  run phitex as `pdflatex`, `latex`, `pdftex`, `etex` or `tex` (a link to
  it), or `phitex --compat=NAME …`: web2c's options, one run."
    )
}

#[derive(Parser, Debug)]
#[command(
    name = "phitex",
    bin_name = "phitex",
    version = env!("CARGO_PKG_VERSION"),
    about = "PhiTeX: TeX documents as graph reduction",
    long_about = ABOUT,
    after_help = after_help(),
    styles = STYLES,
    color = ColorChoice::Auto,
    infer_subcommands = true,
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Sub>,
    #[command(flatten)]
    opts: Opts,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Build the document to its fixpoint (BibTeX and makeindex included)
    Build {
        #[arg(value_name = "FILE")]
        file: Option<String>,
    },
    /// Build, then rebuild whenever an input changes (an edit of the
    /// graph), the pages live in the browser; keys on standard input:
    /// r rebuild, q quit
    Watch {
        #[arg(value_name = "FILE")]
        file: Option<String>,
    },
    /// Build without writing the outputs
    Check {
        #[arg(value_name = "FILE")]
        file: Option<String>,
    },
    /// Remove the files a build writes
    Clean {
        /// The cache of formats and TeX Live's links too (no file needed)
        #[arg(long)]
        all: bool,
        #[arg(value_name = "FILE")]
        file: Option<String>,
    },
    /// FILE:LINE[:COL]: the watch's viewer shows that line. `--pdf PAGE X Y
    /// [FILE.tex]`: the source of what is at (X, Y) (big points from the
    /// page's top left); `--tex FILE LINE [FILE.tex]`: where that line is
    /// shown; both from <job>.synctex.gz, printed as TeX Live's `synctex`
    #[command(verbatim_doc_comment)]
    Sync {
        #[arg(
            value_name = "ARGS",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        args: Vec<String>,
    },
    /// The document's structure, read from its source without TeX
    /// (`phitex outline --help`)
    Outline {
        #[arg(
            value_name = "ARGS",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        args: Vec<String>,
    },
}

/// The options, accepted anywhere on the line.
#[derive(Args, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
struct Opts {
    /// Where the outputs go (TeX's -output-directory)
    #[arg(
        short = 'o',
        long = "output-dir",
        alias = "output-directory",
        alias = "out-dir",
        value_name = "DIR",
        global = true
    )]
    output_dir: Option<String>,
    /// pdflatex (default for LaTeX), latex, pdftex (default otherwise),
    /// etex, tex; lualatex runs the system's lualatex: NOT incremental
    #[arg(long, alias = "program", value_name = "NAME", global = true)]
    engine: Option<String>,
    /// dvi, pdf or xdv (xdv: xelatex's -no-pdf)
    #[arg(long, value_name = "FORMAT", value_parser = ["dvi", "pdf", "xdv"], global = true)]
    output_format: Option<String>,
    /// The file is the whole job: no format (TeX's -ini)
    #[arg(long, global = true)]
    ini: bool,
    /// With --ini: e-TeX's extended mode
    #[arg(long, global = true)]
    etex: bool,
    /// Threads (default: all cores)
    #[arg(
        short = 'j',
        long = "jobs",
        alias = "threads",
        value_name = "N",
        global = true
    )]
    jobs: Option<usize>,
    /// batchmode (default), nonstopmode, scrollmode or errorstopmode
    #[arg(long, value_name = "MODE", value_parser = ["batchmode", "nonstopmode", "scrollmode", "errorstopmode"], global = true)]
    interaction: Option<String>,
    /// Each run, each warning's first lines, an engine failure's whole
    /// message (-vv: more)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    /// Only problems and the result
    #[arg(short, long, global = true)]
    quiet: bool,
    /// watch: open the viewer in the browser
    #[arg(long, global = true)]
    open: bool,
    /// watch: serve the live viewer (the default on a terminal)
    #[arg(long, global = true, overrides_with = "no_view")]
    view: bool,
    /// watch: no live viewer
    #[arg(long, global = true, overrides_with = "view")]
    no_view: bool,
    /// watch: where a double-click on a page opens its source (code,
    /// emacsclient, nvim, … or a command with {file} {line} {col}; else
    /// PHITEX_EDITOR, VISUAL, EDITOR)
    #[arg(long, value_name = "CMD", global = true)]
    editor: Option<String>,
    /// Copy the PDF after each successful build into DIR (default: where
    /// phitex was run)
    #[arg(long, value_name = "DIR", num_args = 0..=1, require_equals = true, default_missing_value = "", global = true)]
    copy_pdf: Option<String>,
    #[arg(long, global = true, hide = true)]
    no_copy_pdf: bool,
    /// Write <job>.synctex.gz (the default; --synctex=0: not)
    #[arg(long, value_name = "N", num_args = 0..=1, require_equals = true, default_missing_value = "1", global = true)]
    synctex: Option<String>,
    /// No <job>.synctex.gz
    #[arg(long, global = true)]
    no_synctex: bool,
    /// Write <job>.fls
    #[arg(long, global = true)]
    recorder: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Build,
    Watch,
    Check,
    Clean,
    Sync,
    Outline,
}

/// The modern command line's options, as the rest of the program reads
/// them.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Options {
    pub command: Option<Command>,
    pub file: Option<String>,
    pub engine: Option<String>,
    pub output: Option<Output>,
    pub ini: bool,
    pub etex: bool,
    pub output_dir: Option<String>,
    pub threads: Option<usize>,
    pub verbose: u8,
    pub quiet: bool,
    pub open: bool,
    pub view: Option<bool>,
    pub editor: Option<String>,
    pub copy_pdf: Option<config::CopyPdf>,
    pub synctex: Option<bool>,
    pub recorder: bool,
    pub all: bool,
}

impl Cli {
    fn into_options(self) -> Options {
        let (command, file, all) = match self.command {
            Some(Sub::Build { file }) => (Some(Command::Build), file, false),
            Some(Sub::Watch { file }) => (Some(Command::Watch), file, false),
            Some(Sub::Check { file }) => (Some(Command::Check), file, false),
            Some(Sub::Clean { all, file }) => (Some(Command::Clean), file, all),
            Some(Sub::Sync { args }) => (Some(Command::Sync), args.into_iter().next(), false),
            Some(Sub::Outline { args }) => (Some(Command::Outline), args.into_iter().next(), false),
            None => (None, None, false),
        };
        let o = self.opts;
        Options {
            command,
            file,
            engine: o.engine,
            output: o.output_format.as_deref().map(|f| match f {
                "dvi" => Output::Dvi,
                "xdv" => Output::Xdv,
                _ => Output::Pdf,
            }),
            ini: o.ini,
            etex: o.etex,
            output_dir: o.output_dir,
            threads: o.jobs,
            verbose: o.verbose,
            quiet: o.quiet,
            open: o.open,
            view: if o.view {
                Some(true)
            } else if o.no_view {
                Some(false)
            } else {
                None
            },
            editor: o.editor,
            copy_pdf: if o.no_copy_pdf {
                Some(config::CopyPdf::Off)
            } else {
                o.copy_pdf.map(|d| {
                    if d.is_empty() {
                        config::CopyPdf::Invocation
                    } else {
                        config::CopyPdf::Dir(d)
                    }
                })
            },
            synctex: if o.no_synctex {
                Some(false)
            } else {
                o.synctex.map(|v| v.trim() != "0")
            },
            recorder: o.recorder,
            all,
        }
    }
}

/// The command line's options (`args` after the program's name). The
/// letters `b`, `w` and `c` are `build`, `watch` and `check`.
fn parse(args: &[String]) -> Result<Options, clap::Error> {
    let first = match args.first().map(String::as_str) {
        Some("b") => Some("build"),
        Some("w") => Some("watch"),
        Some("c") => Some("check"),
        other => other,
    };
    let line = std::iter::once("phitex")
        .chain(first)
        .chain(args.iter().skip(1).map(String::as_str));
    Cli::try_parse_from(line).map(Cli::into_options)
}

/// What to build.
#[derive(Debug)]
pub struct Target {
    /// The main file, as TeX is given it (from the directory the build
    /// runs in).
    pub file: String,
    pub engine: tex::Engine,
    pub output_dir: Option<String>,
    /// Where to copy the PDF after a successful build.
    pub copy_pdf: Option<PathBuf>,
    pub synctex: bool,
    /// An engine run as the system's program, outside the graph
    /// (`external.rs`: LuaLaTeX), by its program name.
    pub external: Option<&'static str>,
}

impl Target {
    pub fn job(&self) -> String {
        crate::compat::job_of(&self.file)
    }
}

/// Resolve what to build: the command line, then `phitex.toml` (running
/// in its directory), then the file's magic comments. Without a
/// `phitex.toml` the build runs in the main file's directory, as latexmk
/// -cd does.
pub(crate) fn resolve(o: &Options) -> Result<Target, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let found = config::find(&cwd)?;
    let (root, cfg, configured) = match (&o.file, found) {
        (None, Some((root, cfg))) => {
            std::env::set_current_dir(&root)
                .map_err(|e| format!("can't enter {}: {e}", root.display()))?;
            (root, cfg, true)
        }
        (Some(_), Some((root, cfg))) if root == cwd => (root, cfg, true),
        _ => (cwd.clone(), config::Config::default(), false),
    };
    let copy_pdf = match (&o.copy_pdf, &cfg.copy_pdf) {
        (Some(c), _) => copy_dir(c, &cwd, &cwd),
        (None, Some(c)) => copy_dir(c, &cwd, &root),
        (None, None) => None,
    };
    let mut file =
        o.file.clone().or_else(|| cfg.main.clone()).ok_or(
            "no file given, and no phitex.toml names one (`main = \"paper.tex\"`)".to_owned(),
        )?;
    let read = |f: &str| {
        std::fs::read(f)
            .or_else(|_| std::fs::read(format!("{f}.tex")))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| format!("can't read {f}: {e}"))
    };
    let mut text = read(&file)?;
    let (mut program, root_comment) = config::magic_comments(&text);
    if let Some(r) = root_comment {
        let dir = Path::new(&file).parent().unwrap_or(Path::new(""));
        file = dir.join(r).to_string_lossy().into_owned();
        text = read(&file)?;
        program = config::magic_comments(&text).0.or(program);
    }
    let named = o.engine.clone().or(cfg.engine).or(program);
    let external = named.as_deref().and_then(external_program);
    let mut engine = match named {
        Some(_) if external.is_some() => tex::engine("pdflatex")?,
        Some(e) => tex::engine(&e)?,
        None => tex::engine(config::default_engine(&text))?,
    };
    if let Some(out) = o.output {
        engine = engine.with_output(out)?;
    }
    let mut output_dir = o.output_dir.clone().or(cfg.output_dir);
    if !configured
        && let Some(dir) = Path::new(&file)
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
    {
        let dir = dir.to_path_buf();
        output_dir = output_dir.map(|d| cwd.join(d).to_string_lossy().into_owned());
        std::env::set_current_dir(&dir)
            .map_err(|e| format!("can't enter {}: {e}", dir.display()))?;
        file = Path::new(&file)
            .file_name()
            .map_or(file.clone(), |f| f.to_string_lossy().into_owned());
    }
    Ok(Target {
        file,
        engine,
        output_dir,
        copy_pdf,
        // (on unless asked otherwise; an external engine writes its own)
        synctex: o.synctex.or(cfg.synctex).unwrap_or(true),
        external,
    })
}

/// The program an external engine's name runs (none: not external; never
/// in a wasm build).
fn external_program(name: &str) -> Option<&'static str> {
    #[cfg(not(target_family = "wasm"))]
    {
        crate::external::program_of(name)
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = name;
        None
    }
}

fn copy_dir(c: &config::CopyPdf, cwd: &Path, base: &Path) -> Option<PathBuf> {
    match c {
        config::CopyPdf::Off => None,
        config::CopyPdf::Invocation => Some(cwd.to_path_buf()),
        config::CopyPdf::Dir(d) => Some(base.join(d)),
    }
}

/// The session job for target `t`.
pub fn job(t: &Target, o: &Options) -> Result<Job, String> {
    let path = crate::compat::find_main(&t.file).ok_or_else(|| format!("can't find {}", t.file))?;
    Ok(Job {
        engine: t.engine,
        ini: o.ini,
        etex: o.etex,
        file: t.file.clone(),
        path,
        jobname: t.job(),
        out_dir: t
            .output_dir
            .as_ref()
            .map_or_else(|| PathBuf::from("."), PathBuf::from),
        // (batch mode, as the oracles run: the CLI shows the errors itself)
        threads: o.threads.unwrap_or_else(crate::default_threads),
        synctex: t.synctex,
        recorder: o.recorder,
        pdf_date: session::pdf_date(),
    })
}

/// What the spinner says is built: `pdflatex paper.tex`.
pub fn what(t: &Target, job: &Job) -> String {
    if job.ini {
        format!("{} (ini)", t.file)
    } else {
        format!("{} {}", t.engine.name, t.file)
    }
}

/// What a build did, for the terminal: each run (verbose), the tools, the
/// errors as reports, the warnings, the result line.
pub fn report(
    t: &Target,
    s: &Session,
    r: &Report,
    wrote: Option<&Path>,
    o: &Options,
    ui: &Ui,
    checked: bool,
) {
    let st = ui.st;
    let job = &s.job;
    if o.verbose > 0 {
        for (k, run) in r.runs.iter().enumerate() {
            ui.line(&st.dim(&format!(
                "  run {}: {} ({}): {} nodes reduced, {} pages ({} kept)",
                k + 1,
                ms(run.wall),
                run.why,
                run.commands_run,
                run.pages,
                run.pages_reused
            )));
        }
    }
    if !o.quiet {
        for n in &r.notes {
            ui.line(&st.dim(&format!("  {n}")));
        }
    }
    let log = job.out_dir.join(format!("{}.log", job.jobname));
    let text = r.log_text();
    crate::problems::show(ui, &r.out.errors, &text, &t.file);
    if r.out.errors.len() > 10 && o.verbose == 0 {
        ui.line(&format!(
            "  … {} errors in all (-v shows more; the log: {})",
            r.out.errors.len(),
            log.display()
        ));
    }
    if !r.unsettled.is_empty() {
        ui.warning(&format!(
            "the written files did not settle in {} runs: {} still changed (oscillating?)",
            r.runs.len(),
            r.unsettled.join(", ")
        ));
    }
    let warnings: usize = crate::warnings::summarize(&text).iter().map(|g| g.1).sum();
    let warn = crate::warnings::lines(&text, o.verbose > 0);
    if !warn.is_empty() && !o.quiet {
        ui.line(&format!(
            "{}: {} ({})",
            st.yellow("warnings"),
            warn.iter()
                .filter(|l| !l.starts_with(' '))
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            log.display()
        ));
        for l in warn.iter().filter(|l| l.starts_with(' ')) {
            ui.line(&st.dim(l));
        }
    }
    ui.result(&crate::ui::Outcome {
        output: wrote,
        file: &t.file,
        pages: r.pages(),
        bytes: r.main_output().map(|(_, b)| b.len()),
        wall: r.wall,
        errors: r.out.errors.len(),
        warnings,
        runs: r.runs.len(),
        engine: (!job.ini).then_some(t.engine.name),
        checked,
    });
}

/// Copy the written PDF into `dir`.
fn copy_pdf(main: &Path, dir: &Path, ui: &Ui) {
    if main.extension().is_none_or(|e| e != "pdf") {
        return;
    }
    let Some(name) = main.file_name() else { return };
    let to = dir.join(name);
    if let Err(e) = std::fs::copy(main, &to) {
        ui.warning(&format!("can't copy the PDF to {}: {e}", to.display()));
    }
}

/// `phitex build` / `phitex check`: one session to the fixpoint. The exit
/// status.
fn build(t: &Target, o: &Options, ui: &Ui, write: bool) -> i32 {
    let job = match job(t, o) {
        Ok(j) => j,
        Err(e) => {
            ui.error(&e);
            return 2;
        }
    };
    for n in session::waiting_notes(&job) {
        ui.line(&format!("{}: {n}", ui.st.yellow("note")));
    }
    let status = Arc::new(Status::default());
    let spin = ui.spin(what(t, &job), status.clone());
    let mut s = match Session::new(job, status.clone(), &mut |n: &str| ui.note(n)) {
        Ok(s) => s,
        Err(e) => {
            spin.done();
            ui.error(&e);
            return 1;
        }
    };
    let r = match crate::ui::catch(|| s.build()) {
        Ok(r) => r,
        Err(m) => {
            spin.done();
            ui.failed("the build", &m);
            return 1;
        }
    };
    let mut main = None;
    if write {
        status.set("writing the outputs");
        match s.write(&r) {
            Ok(w) => main = w.main,
            Err(e) => {
                spin.done();
                ui.error(&e);
                return 1;
            }
        }
    }
    spin.done();
    report(t, &s, &r, main.as_deref(), o, ui, !write);
    if let (Some(m), Some(d), true) = (&main, &t.copy_pdf, r.out.errors.is_empty()) {
        copy_pdf(m, d, ui);
    }
    i32::from(!r.out.errors.is_empty())
}

/// The files a build of job `job` writes (`clean`).
const CLEAN: &[&str] = &[
    "pdf",
    "dvi",
    "xdv",
    "log",
    "fls",
    "synctex.gz",
    "aux",
    "toc",
    "lof",
    "lot",
    "out",
    "bbl",
    "blg",
    "idx",
    "ind",
    "ilg",
    "nav",
    "snm",
    "vrb",
    "bcf",
    "run.xml",
    "glo",
    "gls",
    "glg",
    "loa",
    "thm",
];

fn clean(t: &Target, ui: &Ui) -> i32 {
    let dir = t
        .output_dir
        .as_ref()
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let mut n = 0;
    for ext in CLEAN {
        let p = dir.join(format!("{}.{ext}", t.job()));
        if std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    ui.line(&ui.st.green(&format!(
        "removed {n} file{}",
        if n == 1 { "" } else { "s" }
    )));
    0
}

fn clean_all(ui: &Ui) -> ! {
    let d = tex::cache_dir();
    match std::fs::remove_dir_all(&d) {
        Ok(()) => ui.line(&ui.st.green(&format!("removed {}", d.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ui.line(&format!("nothing cached ({})", d.display()));
        }
        Err(e) => {
            ui.error(&format!("{}: {e}", d.display()));
            std::process::exit(1);
        }
    }
    std::process::exit(0);
}

/// Run the modern command line with `args`.
pub fn main(args: &[String]) -> ! {
    // (its own command line: `outline`'s options)
    if args.first().is_some_and(|a| a == "outline") {
        crate::outline::main(&args[1..]);
    }
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => e.exit(),
    };
    let ui = Ui::new(Style::detect(), o.verbose, o.quiet);
    crate::ui::capture_panics(o.verbose > 0);
    if o.all {
        clean_all(&ui);
    }
    let Some(command) = o.command else {
        ui.error("no command given: build, watch, check, clean, sync or outline (`phitex --help`)");
        std::process::exit(2);
    };
    match command {
        Command::Sync => {
            let Some(place) = o.file.as_deref() else {
                ui.error("sync: give FILE:LINE[:COL]");
                std::process::exit(2);
            };
            std::process::exit(crate::view::sync_command(place));
        }
        Command::Outline => {
            let rest: Vec<String> = args
                .iter()
                .skip_while(|a| *a != "outline")
                .skip(1)
                .cloned()
                .collect();
            crate::outline::main(&rest);
        }
        _ => {}
    }
    let t = resolve(&o).unwrap_or_else(|e| {
        ui.error(&e);
        std::process::exit(2);
    });
    #[cfg(not(target_family = "wasm"))]
    if let Some(p) = t.external {
        match command {
            Command::Build | Command::Check => {
                std::process::exit(crate::external::main_build(p, &t, &o, ui.st))
            }
            Command::Watch => crate::external::main_watch(p, &t, &o, ui.st),
            _ => {}
        }
    }
    match command {
        Command::Build => std::process::exit(build(&t, &o, &ui, true)),
        Command::Check => std::process::exit(build(&t, &o, &ui, false)),
        Command::Clean => std::process::exit(clean(&t, &ui)),
        Command::Watch => crate::watch::main(&t, &o, &ui),
        Command::Sync | Command::Outline => unreachable!("answered above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn options() {
        let o = parse(&v(&["build", "-j", "4", "--output-format=dvi", "x.tex"])).unwrap();
        assert_eq!(o.command, Some(Command::Build));
        assert_eq!(o.threads, Some(4));
        assert_eq!(o.output, Some(Output::Dvi));
        assert_eq!(o.file.as_deref(), Some("x.tex"));
        assert!(parse(&v(&["build", "--frob"])).is_err());
        let o = parse(&v(&["watch", "--no-view", "--ini", "--etex", "a.tex"])).unwrap();
        assert!(o.ini && o.etex);
        assert_eq!(o.view, Some(false));
        assert_eq!(
            parse(&v(&["b", "x.tex"])).unwrap().command,
            Some(Command::Build)
        );
    }

    #[test]
    fn options_anywhere() {
        for line in [
            ["-o", "out", "-j", "2", "watch", "f.tex"],
            ["watch", "-o", "out", "-j", "2", "f.tex"],
            ["watch", "f.tex", "-o", "out", "-j", "2"],
        ] {
            let o = parse(&v(&line)).unwrap();
            assert_eq!(o.command, Some(Command::Watch), "{line:?}");
            assert_eq!(o.output_dir.as_deref(), Some("out"), "{line:?}");
            assert_eq!(o.threads, Some(2), "{line:?}");
            assert_eq!(o.file.as_deref(), Some("f.tex"), "{line:?}");
        }
        let o = parse(&v(&["build", "--copy-pdf", "--synctex=0", "-vv", "x.tex"])).unwrap();
        assert_eq!(o.verbose, 2);
        assert_eq!(parse(&v(&["-v", "build", "x.tex"])).unwrap().verbose, 1);
        assert!(matches!(o.copy_pdf, Some(config::CopyPdf::Invocation)));
        assert_eq!(o.synctex, Some(false));
        assert_eq!(o.file.as_deref(), Some("x.tex"));
        let e = parse(&v(&["build", "--frob", "x.tex"]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("--frob"), "{e}");
        assert!(parse(&v(&["clean", "--all"])).unwrap().all);
        assert_eq!(
            parse(&v(&["sync", "a.tex:3"])).unwrap().file.as_deref(),
            Some("a.tex:3")
        );
    }
}
