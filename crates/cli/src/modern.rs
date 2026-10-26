//! The modern command line: `phitex build`, `phitex watch`, `phitex
//! clean` (clap: the options anywhere on the line, a coloured sectioned
//! `--help`, typos suggested). What to build comes from the command line, `phitex.toml`
//! and the main file's magic comments (`config.rs`); the build is a
//! a document session (`session.rs`), to the cross-run fixpoint,
//! BibTeX and makeindex included; the terminal is `ui.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::builder::styling::{AnsiColor, Styles};
use clap::{Args, ColorChoice, Parser, Subcommand};

use serde_json::json;

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

A document is built to its fixpoint as latexmk builds it with pdflatex:
the files it writes and reads back go round until they settle (at most
5 rounds, latexmk's limit, then the last round's output and a warning),
BibTeX, biber and makeindex run in process when the document reads what
they make. Under `watch` it is kept up to date as its files and the
editor's buffers change, each build byte-identical to a cold one, the
pages live in the browser. It never stops for input, and never runs an
external TeX, BibTeX or \\write18 command.

Exit status, as pdflatex's: 0 built without errors (warnings allowed),
1 anything else (TeX's errors, a usage, configuration or file error, or
PhiTeX itself failing; standard error says which).";

/// The sections after the options (styled as clap's headers; the escapes
/// are stripped when colour is off).
fn after_help() -> String {
    let h = STYLES.get_header();
    let l = STYLES.get_literal();
    format!(
        "{h}Examples:{h:#}
  {l}phitex build paper.tex{l:#}              the PDF beside it, pdflatex for LaTeX
  {l}phitex watch -o out paper.tex{l:#}       rebuild on each change, the viewer in the browser
  {l}phitex build --json paper.tex{l:#}       the build's events as JSON lines
  {l}phitex clean paper.tex{l:#}              remove its outputs and its part of the cache
  {l}phitex clean --all{l:#}                  remove the whole cache, every document's

{h}Configuration:{h:#}
  Without a file, `phitex.toml` (here or above) names it:
      main = \"paper.tex\"   engine = \"pdflatex\"   output-dir = \"out\"
      copy-pdf = true   (or a directory, relative to phitex.toml's)
      synctex = false   shell-escape = true   editor = \"code\"   viewer = \"zathura\"
      cache-limit = \"4GB\"   (0: no cache; or PHITEX_CACHE_LIMIT)
  ~/.config/phitex/phitex.toml holds your own defaults; a project's
  phitex.toml wins over it key by key.
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
    /// Build the document to its fixpoint (BibTeX, biber and makeindex in
    /// process)
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
    /// Remove the files a build writes
    Clean {
        /// The whole cache, every document's (no file needed)
        #[arg(long)]
        all: bool,
        #[arg(value_name = "FILE")]
        file: Option<String>,
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
    /// etex, tex (xelatex and lualatex are not supported yet)
    #[arg(long, alias = "program", value_name = "NAME", global = true)]
    engine: Option<String>,
    /// dvi or pdf
    #[arg(long, value_name = "FORMAT", value_parser = ["dvi", "pdf"], global = true)]
    output_format: Option<String>,
    /// The file is the whole job: no format (TeX's -ini)
    #[arg(long, global = true)]
    ini: bool,
    /// With --ini: e-TeX's extended mode
    #[arg(long, global = true)]
    etex: bool,
    /// The engine's threads (default: PHITEX_THREADS, else every logical
    /// CPU, hyperthreads included); the output is the same at any count
    #[arg(
        short = 'j',
        long = "jobs",
        alias = "threads",
        value_name = "N",
        global = true
    )]
    jobs: Option<usize>,
    /// watch: every edit writes the exact PDF file too (else an edit
    /// appends an update section to it, and the exact file is written
    /// once the editor is idle)
    #[arg(long, global = true)]
    exact: bool,
    /// (the same as --exact)
    #[arg(long = "exact-pdf", global = true, hide = true)]
    exact_pdf: bool,
    /// Whether the engine runs steps ahead of their inputs: on those an
    /// earlier run predicts (the default), never, or on any (not usable
    /// yet). The output is the same either way.
    #[arg(long = "spec", value_name = "WHEN", value_parser = ["off", "predicted", "all"], global = true)]
    spec: Option<String>,
    /// A long piece's readers do not wait for it on the fields an earlier
    /// build learned it defines last before them (they do by default: kept
    /// beside the format). The output is the same either way.
    #[arg(long = "no-wait-long", global = true, hide = true)]
    no_wait_long: bool,
    /// The document nodes' records read and stored (off by default; kept
    /// per document in the cache). The output is the same either way.
    #[arg(long = "node-records", global = true, hide = true)]
    node_records: bool,
    /// Each run, each warning's first lines, an engine failure's whole
    /// message (-vv: more)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    /// Only problems and the result
    #[arg(short, long, global = true)]
    quiet: bool,
    /// The build's events on standard output, one JSON object a line
    /// (nothing else is printed)
    #[arg(long, global = true)]
    json: bool,
    /// \pdfshellescape 1 instead of TeX Live's restricted default (where
    /// \write18 may name bibtex and makeindex): biber is allowed too. The
    /// tools run in process; no other command runs either way
    #[arg(long, global = true)]
    shell_escape: bool,
    /// watch: open the viewer in the browser (or start phitex.toml's
    /// `viewer`, zathura or the like, on the output)
    #[arg(long, global = true)]
    open: bool,
    /// watch: no live viewer
    #[arg(long, global = true)]
    no_view: bool,
    /// watch: the diagnostics at their places after every edit too (the
    /// viewer asks for them; else an edit reports the engine's own only)
    #[arg(long, global = true)]
    diagnostics: bool,
    /// watch: the time without an edit after which the exact PDF file is
    /// written over the updated one (default 300)
    #[arg(long, value_name = "MS", global = true)]
    idle: Option<u64>,
    /// watch: the log written too, with the exact PDF file when idle
    /// (else a watch writes no log)
    #[arg(long, global = true)]
    log: bool,
    /// watch: the viewer's port (default: a free one)
    #[arg(long, value_name = "N", global = true)]
    port: Option<u16>,
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
    /// No <job>.synctex.gz (the default until PhiTeX writes SyncTeX; a
    /// `phitex.toml`'s `synctex = true` asks for it, with a note)
    #[arg(long, global = true)]
    no_synctex: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Build,
    Watch,
    Clean,
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
    pub spec: Option<String>,
    pub wait_long: bool,
    pub node_records: bool,
    pub verbose: u8,
    pub quiet: bool,
    pub json: bool,
    pub shell_escape: bool,
    pub open: bool,
    pub view: bool,
    pub diagnostics: bool,
    /// watch: every edit writes the exact PDF file (`--exact`).
    pub exact_file: bool,
    /// watch: the idle time before the exact file is written.
    pub idle: Duration,
    /// watch: the log written with the exact file (`--log`).
    pub log: bool,
    pub port: Option<u16>,
    pub editor: Option<String>,
    pub copy_pdf: Option<config::CopyPdf>,
    pub synctex: Option<bool>,
    pub all: bool,
}

impl Cli {
    fn into_options(self) -> Options {
        let (command, file, all) = match self.command {
            Some(Sub::Build { file }) => (Some(Command::Build), file, false),
            Some(Sub::Watch { file }) => (Some(Command::Watch), file, false),
            Some(Sub::Clean { all, file }) => (Some(Command::Clean), file, all),
            None => (None, None, false),
        };
        let o = self.opts;
        Options {
            command,
            file,
            engine: o.engine,
            output: o.output_format.as_deref().map(|f| match f {
                "dvi" => Output::Dvi,
                _ => Output::Pdf,
            }),
            ini: o.ini,
            etex: o.etex,
            output_dir: o.output_dir,
            threads: o.jobs,
            spec: o.spec,
            wait_long: !o.no_wait_long,
            node_records: o.node_records,
            verbose: o.verbose,
            quiet: o.quiet,
            json: o.json,
            shell_escape: o.shell_escape,
            open: o.open,
            view: !o.no_view,
            diagnostics: o.diagnostics,
            exact_file: o.exact || o.exact_pdf,
            idle: Duration::from_millis(o.idle.unwrap_or(300)),
            log: o.log,
            port: o.port,
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
            synctex: o.no_synctex.then_some(false),
            all,
        }
    }
}

/// The command line's options (`args` after the program's name).
fn parse(args: &[String]) -> Result<Options, clap::Error> {
    let line = std::iter::once("phitex").chain(args.iter().map(String::as_str));
    Cli::try_parse_from(line).map(Cli::into_options)
}

/// What to build.
#[derive(Clone, Debug)]
pub struct Target {
    /// The main file, as TeX is given it (from the directory the build
    /// runs in).
    pub file: String,
    pub engine: tex::Engine,
    pub output_dir: Option<String>,
    /// Where to copy the PDF after a successful build.
    pub copy_pdf: Option<PathBuf>,
    pub synctex: bool,
    /// `\write18` runs commands.
    pub shell_escape: bool,
    /// watch: the editor a double-click opens (`--editor`, `phitex.toml`).
    pub editor: Option<String>,
    /// watch: a program `--open` starts on the output instead of the
    /// browser (`viewer` in `phitex.toml`).
    pub viewer: Option<String>,
    /// The cache's limit (`cache-limit`; none: the default).
    pub cache_limit: Option<u64>,
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
/// The bytes of the main file its magic comments are looked for in
/// (their twenty lines, [`config::magic_comments`]).
const MAGIC_HEAD: u64 = 1 << 16;

pub(crate) fn resolve(o: &Options) -> Result<Target, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let found = config::find(&cwd)?;
    let user = config::user()?;
    let (root, cfg, configured) = match (&o.file, found) {
        (None, Some((root, cfg))) => {
            std::env::set_current_dir(&root)
                .map_err(|e| format!("can't enter {}: {e}", root.display()))?;
            (root, cfg, true)
        }
        (Some(_), Some((root, cfg))) if root == cwd => (root, cfg, true),
        _ => (cwd.clone(), config::Config::default(), false),
    };
    // (the project's settings over the user's, key by key)
    let cfg = cfg.over(user);
    let copy_pdf = match (&o.copy_pdf, &cfg.copy_pdf) {
        (Some(c), _) => copy_dir(c, &cwd, &cwd),
        (None, Some(c)) => copy_dir(c, &cwd, &root),
        (None, None) => None,
    };
    let mut file =
        o.file.clone().or_else(|| cfg.main.clone()).ok_or(
            "no file given, and no phitex.toml names one (`main = \"paper.tex\"`)".to_owned(),
        )?;
    // (the magic comments are in the first lines: the whole file is read
    // only when no engine is named and its text decides)
    let read = |f: &str, whole: bool| {
        let open = |p: &str| -> std::io::Result<Vec<u8>> {
            if whole {
                return std::fs::read(p);
            }
            let mut b = Vec::new();
            std::io::Read::read_to_end(
                &mut std::io::Read::take(std::fs::File::open(p)?, MAGIC_HEAD),
                &mut b,
            )?;
            // (a head without its twenty whole lines: the whole file)
            if b.len() as u64 == MAGIC_HEAD && b.split(|&c| c == b'\n').nth(20).is_none() {
                return std::fs::read(p);
            }
            Ok(b)
        };
        open(f)
            .or_else(|_| open(&format!("{f}.tex")))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| format!("can't read {f}: {e}"))
    };
    let mut text = read(&file, false)?;
    let (mut program, root_comment) = config::magic_comments(&text);
    if let Some(r) = root_comment {
        let dir = Path::new(&file).parent().unwrap_or(Path::new(""));
        file = dir.join(r).to_string_lossy().into_owned();
        text = read(&file, false)?;
        program = config::magic_comments(&text).0.or(program);
    }
    let named = o.engine.clone().or(cfg.engine).or(program);
    let mut engine = if let Some(e) = named {
        tex::engine(&e)?
    } else {
        let t = read(&file, true)?;
        let mut e = config::default_engine(&t);
        // (a file that inputs a LaTeX document is one: a second
        // edition's main file setting a switch before its first's)
        if e == "pdftex" {
            let dir = Path::new(&file).parent().unwrap_or(Path::new(""));
            for name in config::braced_inputs(&t) {
                let f = dir.join(name).to_string_lossy().into_owned();
                if read(&f, true).is_ok_and(|u| config::default_engine(&u) == "pdflatex") {
                    e = "pdflatex";
                    break;
                }
            }
        }
        tex::engine(e)?
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
        // (off unless asked for: PhiTeX writes no SyncTeX yet, and a
        // build that asks for it says so)
        synctex: o.synctex.or(cfg.synctex).unwrap_or(false),
        shell_escape: o.shell_escape || cfg.shell_escape.unwrap_or(false),
        editor: o.editor.clone().or(cfg.editor),
        viewer: cfg.viewer,
        cache_limit: cfg.cache_limit,
    })
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
        threads: o.threads,
        spec: o.spec.clone(),
        wait_long: o.wait_long,
        node_records: o.node_records,
        synctex: t.synctex,
        recorder: false,
        shell_escape: t.shell_escape,
        pdf_date: session::pdf_date(&t.engine),
        // (the files the job reads back, .aux and .toc, to their fixpoint)
        fixpoint: true,
        cache_limit: t.cache_limit,
        edit_diagnostics: o.diagnostics || o.view,
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
/// With `summary`, the `build` event and the result line too (a watch
/// prints its own).
pub fn report(
    t: &Target,
    s: &Session,
    r: &Report,
    wrote: Option<&Path>,
    o: &Options,
    ui: &Ui,
    summary: bool,
) {
    let st = ui.st;
    let job = &s.job;
    if o.verbose > 0 {
        for (k, run) in r.runs.iter().enumerate() {
            ui.line(&st.dim(&format!(
                "  run {}: {} ({}): {} nodes reduced{}",
                k + 1,
                ms(run.wall),
                run.why,
                run.commands_run,
                run.parallel().map(|p| format!(", {p}")).unwrap_or_default(),
            )));
        }
    }
    if !o.quiet {
        for n in &r.notes {
            ui.line(&st.dim(&format!("  {n}")));
        }
    }
    if let (Some(d), true) = (&r.internal, o.verbose > 0 || ui.verbose > 0) {
        ui.line(&st.dim(&format!("  the engine's details: {d}")));
    }
    let log = job.out_dir.join(format!("{}.log", job.jobname));
    let text = r.log_text();
    // (the engine's failures — a deadline, no fixpoint, an evaluation
    // error — each on its own line first: the result is not the
    // document's, whatever TeX's errors and pages say)
    for f in r.out.errors.iter().filter(|e| !e.starts_with("! ")) {
        ui.error(f);
    }
    // (the engine's places when it gave them, else the transcript's)
    let placed = crate::problems::rendered(s, r, false, st.color, o.verbose > 0 || ui.verbose > 0);
    if placed.is_empty() {
        crate::problems::show(ui, &r.out.errors, &text, &t.file);
    } else {
        let max = if o.verbose > 0 || ui.verbose > 0 {
            100
        } else {
            10
        };
        for p in placed.iter().take(max) {
            ui.line(p);
        }
    }
    if r.out.errors.len() > 10 && o.verbose == 0 {
        ui.line(&format!(
            "  … {} errors in all (-v shows more; the log: {})",
            r.out.errors.len(),
            log.display()
        ));
    }
    for w in &r.warnings {
        ui.warning(w);
    }
    let warnings: usize = crate::warnings::summarize(&text).iter().map(|g| g.1).sum();
    let run = r.runs.last().cloned().unwrap_or_default();
    if summary {
        ui.event(&json!({
        "event": "build",
        "why": run.why,
        "ok": r.out.errors.is_empty(),
        "output": wrote.map(|p| p.display().to_string()),
        "pages": r.pages(),
        "wall_ms": r.wall.as_secs_f64() * 1e3,
        "nodes_run": run.commands_run,
        "rounds": run.rounds,
        "errors": r.out.errors,
        "warnings": warnings,
        "diagnostics": r.engine_diagnostics(),
        "engine_warnings": r.warnings,
        "log": log.display().to_string(),
        }));
    }
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
    if !summary {
        return;
    }
    ui.result(&crate::ui::Outcome {
        output: wrote,
        file: &t.file,
        pages: r.pages(),
        bytes: r.main_output().map(|(_, b)| b.len()),
        wall: r.wall,
        errors: r.out.errors.len(),
        warnings,
        diagnostics: r.engine_diagnostics(),
        runs: r.runs.len(),
        engine: (!job.ini).then_some(t.engine.name),
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

/// `phitex build`: one session to the fixpoint. The exit status.
fn build(t: &Target, o: &Options, ui: &Ui) -> i32 {
    let job = match job(t, o) {
        Ok(j) => j,
        Err(e) => {
            ui.error(&e);
            return crate::EXIT_FAILED;
        }
    };
    for n in session::waiting_notes(&job) {
        ui.line(&format!("{}: {n}", ui.st.yellow("note")));
        ui.event(&json!({"event": "note", "message": n}));
    }
    let status = Arc::new(Status::default());
    let spin = ui.spin(what(t, &job), status.clone());
    phitex_tex::timeline::lap("cli: the job, the spinner");
    let mut s = match Session::new(job, status.clone(), &mut |n: &str| ui.note(n)) {
        Ok(s) => s,
        Err(e) => {
            spin.done();
            ui.error(&e);
            return crate::EXIT_FAILED;
        }
    };
    let r = match crate::ui::catch(|| s.build()) {
        Ok(r) => r,
        Err(m) => {
            spin.done();
            ui.failed("the build", &m);
            return crate::EXIT_FAILED;
        }
    };
    status.set("writing the outputs");
    phitex_tex::timeline::lap("cli: the build returned");
    let main = match s.write(&r, true) {
        Ok(w) => w.main,
        Err(e) => {
            spin.done();
            ui.error(&e);
            return crate::EXIT_FAILED;
        }
    };
    spin.done();
    phitex_tex::timeline::lap("cli: the spinner stopped");
    report(t, &s, &r, main.as_deref(), o, ui, true);
    phitex_tex::timeline::lap("cli: the report");
    if let (Some(m), Some(d), true) = (&main, &t.copy_pdf, r.out.errors.is_empty()) {
        copy_pdf(m, d, ui);
    }
    // (the records the build changed, stored once it is done)
    s.flush_records();
    phitex_tex::timeline::lap("cli: the records stored");
    cache_summary(t, ui);
    phitex_tex::timeline::lap("cli: the cache's summary");
    let code = i32::from(!r.out.errors.is_empty());
    // (the process exits next: the session's engine and tables, hundreds
    // of megabytes, are left to it instead of freed one by one)
    std::mem::forget(s);
    std::mem::forget(r);
    phitex_tex::timeline::end("cold build");
    code
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

/// The cache's line (dim, not with `-q`) and `cache` event: after a
/// build, and when a watch ends.
pub fn cache_summary(t: &Target, ui: &Ui) {
    match crate::cache::Cache::open(t.cache_limit) {
        Ok(c) => {
            if !ui.quiet {
                ui.line(&ui.st.dim(&c.summary()));
            }
            ui.event(&json!({
                "event": "cache",
                "dir": c.dir.display().to_string(),
                "bytes": c.usage(),
                "limit": c.limit,
            }));
        }
        Err(e) => ui.warning(&e),
    }
}

/// `phitex clean`: the document's outputs and its part of the cache.
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
    let freed = match crate::cache::Cache::open(t.cache_limit) {
        Ok(c) => c.clear_doc(Path::new(&t.file)).unwrap_or_else(|e| {
            ui.warning(&format!("the document's cache: {e}"));
            0
        }),
        Err(e) => {
            ui.warning(&e);
            0
        }
    };
    ui.line(&ui.st.green(&format!(
        "removed {n} file{} and {} of cache",
        if n == 1 { "" } else { "s" },
        crate::cache::size(freed)
    )));
    ui.event(&json!({"event": "clean", "removed": n, "cache_bytes": freed}));
    0
}

/// `phitex clean --all`: the whole cache, every document's.
fn clean_all(ui: &Ui) -> ! {
    let c = crate::cache::Cache::open(None).unwrap_or_else(|e| {
        ui.error(&e);
        std::process::exit(crate::EXIT_FAILED);
    });
    let dir = crate::cache::home_relative(&c.dir);
    match c.clear_all() {
        Ok(b) => {
            ui.line(&ui.st.green(&format!(
                "removed {} of cache ({dir})",
                crate::cache::size(b)
            )));
            ui.event(&json!({"event": "clean", "cache_bytes": b}));
        }
        Err(e) => {
            ui.error(&format!("{dir}: {e}"));
            std::process::exit(crate::EXIT_FAILED);
        }
    }
    std::process::exit(0);
}

/// Run the modern command line with `args`.
pub fn main(args: &[String]) -> ! {
    // (clap's own exit status for a usage error is 2: pdflatex's is 1)
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            let _ = e.print();
            std::process::exit(if e.use_stderr() {
                crate::EXIT_FAILED
            } else {
                0
            });
        }
    };
    let ui = Ui::new(Style::detect(), o.verbose, o.quiet, o.json);
    crate::ui::capture_panics(o.verbose > 0);
    if o.all {
        clean_all(&ui);
    }
    // (`PHITEX_TEX_TIME`: a cold build's phases from the process's start)
    if matches!(o.command, Some(Command::Build))
        && let Some(&t0) = crate::START.get()
    {
        phitex_tex::timeline::begin_at(t0);
    }
    let Some(command) = o.command else {
        ui.error("no command given: build, watch or clean (`phitex --help`)");
        std::process::exit(crate::EXIT_FAILED);
    };
    let t = resolve(&o).unwrap_or_else(|e| {
        ui.error(&e);
        std::process::exit(crate::EXIT_FAILED);
    });
    phitex_tex::timeline::lap("cli: the arguments, the target (configuration read)");
    match command {
        Command::Build => std::process::exit(build(&t, &o, &ui)),
        Command::Clean => std::process::exit(clean(&t, &ui)),
        Command::Watch => crate::watch::main(&t, &o, &ui),
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
        assert!(!o.view);
        let o = parse(&v(&["build", "--json", "--shell-escape", "x.tex"])).unwrap();
        assert!(o.json && o.shell_escape && o.view);
        for gone in [
            &["b", "x.tex"][..],
            &["check", "x.tex"],
            &["sync", "a.tex:3"],
            &["outline", "x.tex"],
            &["x.tex"],
            &["build", "--interaction=nonstopmode", "x.tex"],
            &["watch", "--view", "x.tex"],
        ] {
            assert!(parse(&v(gone)).is_err(), "{gone:?}");
        }
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
        let o = parse(&v(&["build", "--copy-pdf", "--no-synctex", "-vv", "x.tex"])).unwrap();
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
    }
}
