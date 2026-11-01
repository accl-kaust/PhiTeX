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
5 rounds, latexmk's limit; past them the build fails as latexmk's does,
the last round's output kept), BibTeX, biber and makeindex run in
process when the document reads what they make. Under `watch` it is kept up to date as its files and the
editor's buffers change, each build byte-identical to a cold one, the
pages live in the browser. It never stops for input, and never runs an
external TeX, BibTeX or \\write18 command.

Exit status, as pdflatex's: 0 built without errors (warnings allowed),
1 anything else (TeX's errors, a usage, configuration or file error, or
PhiTeX itself failing; standard error says which).";

/// `--help`'s last lines: where the settings are, and `--help-all`.
fn short_after_help() -> String {
    let l = STYLES.get_literal();
    format!(
        "Settings: phitex.toml (the project's), ~/.config/phitex/phitex.toml (yours):
  {l}phitex config KEY VALUE{l:#}, e.g. {l}phitex config update-check on{l:#}
Every option: {l}phitex --help-all{l:#} · https://phitex.org"
    )
}

/// `--help-all`'s sections after the options (styled as clap's headers;
/// the escapes are stripped when colour is off).
fn after_help() -> String {
    let h = STYLES.get_header();
    let l = STYLES.get_literal();
    format!(
        "{h}Examples:{h:#}
  {l}phitex build paper.tex{l:#}              the PDF beside it, pdflatex for LaTeX
  {l}phitex watch -o out paper.tex{l:#}       rebuild on each change, the viewer in the browser
  {l}phitex watch --lsp{l:#}                  the same for an editor's LSP client (its buffers, problems, cursor)
  {l}phitex build --shelf paper.tex{l:#}      TeX's files from Shelf's packs, not TeX Live (v1.shelf.phitex.org)
  {l}phitex build --json paper.tex{l:#}       the build's events as JSON lines
  {l}phitex clean paper.tex{l:#}              remove its outputs and its part of the cache
  {l}phitex clean --all{l:#}                  remove the whole cache, every document's

{h}Configuration:{h:#}
  Without a file, `phitex.toml` (here or above) names it:
      main = \"paper.tex\"   engine = \"pdflatex\"   output-dir = \"out\"
      copy-pdf = true   (or a directory, relative to phitex.toml's)
      synctex = false   shell-escape = true   editor = \"code\"   viewer = \"zathura\"
      cache-limit = \"4GB\"   (0: no cache; or PHITEX_CACHE_LIMIT)
      update-check = true   (yours only: a daily check for a newer release)
      files = \"shelf\"   (TeX's files from Shelf's packs, kept in the cache's shelf/;
                          \"texlive\", kpathsea's, the default; one source a run)
  ~/.config/phitex/phitex.toml holds your own defaults; a project's
  phitex.toml wins over it key by key.
  A `% !TEX program = …` or `% !TEX root = …` comment in the file is honoured.

{h}TeX's own command line:{h:#}
  run phitex as `pdflatex`, `latex`, `pdftex`, `etex` or `tex` (a link to
  it), or `phitex --compat=NAME …`: web2c's options, one run."
    )
}

/// `phitex --version`'s text (after `phitex `): the version, its author,
/// the license, and what other versions it works with (docs/spec-distribution.md §6).
pub fn version_text() -> &'static str {
    static TEXT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "{}\n\
             Copyright 2026 Ammar Seliaman (https://github.com/Flinner, flinner <at> nand <dot> sh)\n\
             AGPL-3.0-only: free software, without any warranty. https://phitex.org\n\
             engine {:016x}, records {}, Shelf schema {} (release {}), editor protocol {}",
            crate::version(),
            phitex_tex::engine_id(),
            phitex_live::host::RECORDS_VERSION,
            phitex_shelf::SCHEMAS.map(|s| s.to_string()).join(", "),
            crate::shelf::RELEASE,
            phitex_live::lsp::PROTOCOL,
        )
    });
    &TEXT
}

#[derive(Parser, Debug)]
#[command(
    name = "phitex",
    bin_name = "phitex",
    version = crate::version(),
    long_version = version_text(),
    about = "PhiTeX: LaTeX that updates as you type",
    long_about = ABOUT,
    after_help = short_after_help(),
    after_long_help = after_help(),
    disable_help_flag = true,
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
    /// Build the PDF (its reruns, BibTeX, biber and makeindex included)
    ///
    /// The document is built to its fixpoint as latexmk builds it, the
    /// tools run in process.
    Build {
        #[arg(value_name = "FILE")]
        file: Option<String>,
    },
    /// Rebuild on every change, the pages live in the browser
    ///
    /// Build, then rebuild whenever an input changes (an edit of the
    /// graph); keys on standard input: r rebuild, q quit (with --lsp: an
    /// editor's language client there).
    Watch {
        #[arg(value_name = "FILE")]
        file: Option<String>,
    },
    /// What is new in this version
    News,
    /// Install the latest release
    ///
    /// Its signature checked, for an install made by phitex.org/install.sh.
    Upgrade,
    /// Change one of your settings (phitex config update-check on)
    ///
    /// Written into ~/.config/phitex/phitex.toml, its other lines kept.
    Config {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
    /// Remove a build's files (--all: the whole cache)
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
    /// pdflatex (default), xelatex, latex, pdftex, …
    ///
    /// pdflatex (default for LaTeX), latex, xelatex, pdftex (default
    /// otherwise), xetex, etex, tex (lualatex is not supported yet)
    #[arg(long, alias = "program", value_name = "NAME", global = true)]
    engine: Option<String>,
    /// dvi or pdf (xdv: XeTeX's XDV file, not made a PDF)
    #[arg(long, value_name = "FORMAT", value_parser = ["dvi", "pdf", "xdv"], global = true, hide_short_help = true)]
    output_format: Option<String>,
    /// The file is the whole job: no format (TeX's -ini)
    #[arg(long, global = true, hide_short_help = true)]
    ini: bool,
    /// With --ini: e-TeX's extended mode
    #[arg(long, global = true, hide_short_help = true)]
    etex: bool,
    /// Threads (default: every core); the output is the same at any count
    ///
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
    #[arg(long, global = true, hide_short_help = true)]
    exact: bool,
    /// (the same as --exact)
    #[arg(long = "exact-pdf", global = true, hide = true)]
    exact_pdf: bool,
    /// Whether the engine runs steps ahead of their inputs: on those an
    /// earlier run predicts (the default), never, or on any (not usable
    /// yet). The output is the same either way.
    #[arg(long = "spec", value_name = "WHEN", value_parser = ["off", "predicted", "all"], global = true, hide_short_help = true)]
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
    /// More: each run, the engine's counts, the cache (-vv: more still)
    ///
    /// Each run, each warning's first lines, an engine failure's whole
    /// message, the engine's counts and tables, the cache's use (-vv: more)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    /// Only problems and the result
    #[arg(short, long, global = true)]
    quiet: bool,
    /// The build's events on standard output, one JSON object a line
    /// (nothing else is printed)
    #[arg(long, global = true, hide_short_help = true)]
    json: bool,
    /// \pdfshellescape 1 instead of TeX Live's restricted default (where
    /// \write18 may name bibtex and makeindex): biber is allowed too. The
    /// tools run in process; no other command runs either way
    #[arg(long, global = true, hide_short_help = true)]
    shell_escape: bool,
    /// watch: open the viewer
    ///
    /// watch: open the viewer in the browser (or start phitex.toml's
    /// `viewer`, zathura or the like, on the output)
    #[arg(long, global = true)]
    open: bool,
    /// watch: no live viewer
    #[arg(long, global = true, hide_short_help = true)]
    no_view: bool,
    /// watch: the diagnostics at their places after every edit too (the
    /// viewer asks for them; else an edit reports the engine's own only)
    #[arg(long, global = true, hide_short_help = true)]
    diagnostics: bool,
    /// watch: the time without an edit after which the exact PDF file is
    /// written over the updated one (default 300)
    #[arg(long, value_name = "MS", global = true, hide_short_help = true)]
    idle: Option<u64>,
    /// watch: also write the .log file, each time the watch goes idle
    /// (beside the exact PDF). Without it a watch writes no .log; build
    /// always writes one
    #[arg(long, global = true, hide_short_help = true)]
    log: bool,
    /// watch: be an editor's language server on standard input and
    /// output (LSP): its unsaved buffers built as you type, the problems
    /// shown in it, a double-click on a page opening it there, its cursor
    /// shown on the page (`phitex/cursor`); nothing else on standard output
    #[arg(long, global = true, hide_short_help = true)]
    lsp: bool,
    /// watch: the viewer's port (default: a free one)
    #[arg(long, value_name = "N", global = true, hide_short_help = true)]
    port: Option<u16>,
    /// watch: where a double-click on a page opens its source (code,
    /// emacsclient, nvim, … or a command with {file} {line} {col}; else
    /// PHITEX_EDITOR, VISUAL, EDITOR)
    #[arg(long, value_name = "CMD", global = true, hide_short_help = true)]
    editor: Option<String>,
    /// Copy the PDF after each successful build into DIR (default: where
    /// phitex was run)
    #[arg(long, value_name = "DIR", num_args = 0..=1, require_equals = true, default_missing_value = "", global = true, hide_short_help = true)]
    copy_pdf: Option<String>,
    #[arg(long, global = true, hide = true)]
    no_copy_pdf: bool,
    /// Write <job>.synctex.gz, for a PDF viewer's jumps between source
    /// and pages (`synctex = true` in `phitex.toml`)
    #[arg(
        long,
        global = true,
        overrides_with = "no_synctex",
        hide_short_help = true
    )]
    synctex: bool,
    /// Write <job>.phitex-report.txt beside the outputs, for a bug report
    /// (no text of the document; `--report=full` adds the .log and the
    /// project's sources)
    #[arg(long, value_name = "full", num_args = 0..=1, require_equals = true, default_missing_value = "", value_parser = ["", "full"], global = true, hide_short_help = true)]
    report: Option<String>,
    /// No <job>.synctex.gz (the default)
    #[arg(
        long,
        global = true,
        overrides_with = "synctex",
        hide_short_help = true
    )]
    no_synctex: bool,
    /// TeX's files from Shelf's packs (fetched once, kept in the cache's
    /// `shelf/`; v1.shelf.phitex.org the server, PHITEX_SHELF_URL another) instead of TeX Live's
    /// (`files = "shelf"` in `phitex.toml`)
    #[arg(long, global = true, hide_short_help = true)]
    shelf: bool,
    /// Print help (--help-all: every option)
    #[arg(short = 'h', long = "help", action = clap::ArgAction::HelpShort, global = true)]
    help: Option<bool>,
    /// Print every option, with the details
    #[arg(long = "help-all", action = clap::ArgAction::HelpLong, global = true, hide_short_help = true)]
    help_all: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Build,
    Watch,
    Clean,
    News,
    Upgrade,
    Config,
}

/// The modern command line's options, as the rest of the program reads
/// them.
#[derive(Clone, Debug, Default)]
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
    /// watch: an editor's language server on standard input and output.
    pub lsp: bool,
    pub editor: Option<String>,
    pub copy_pdf: Option<config::CopyPdf>,
    pub synctex: Option<bool>,
    pub all: bool,
    /// `--shelf`, or the editor's `files` setting (none: `phitex.toml`'s,
    /// else TeX Live).
    pub files: Option<tex::Files>,
    /// `config`'s key and value.
    pub set: Option<(String, String)>,
    /// `--report` (true: `=full`).
    pub report: Option<bool>,
}

impl Cli {
    fn into_options(self) -> Options {
        let set = match &self.command {
            Some(Sub::Config { key, value }) => Some((key.clone(), value.clone())),
            _ => None,
        };
        let (command, file, all) = match self.command {
            Some(Sub::Build { file }) => (Some(Command::Build), file, false),
            Some(Sub::Watch { file }) => (Some(Command::Watch), file, false),
            Some(Sub::Clean { all, file }) => (Some(Command::Clean), file, all),
            Some(Sub::News) => (Some(Command::News), None, false),
            Some(Sub::Upgrade) => (Some(Command::Upgrade), None, false),
            Some(Sub::Config { .. }) => (Some(Command::Config), None, false),
            None => (None, None, false),
        };
        let o = self.opts;
        Options {
            command,
            file,
            engine: o.engine,
            output: o.output_format.as_deref().map(|f| match f {
                "dvi" => Output::Dvi,
                "xdv" => Output::XdvFile,
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
            lsp: o.lsp,
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
            synctex: if o.synctex {
                Some(true)
            } else {
                o.no_synctex.then_some(false)
            },
            all,
            files: o.shelf.then_some(tex::Files::Shelf),
            set,
            report: o.report.map(|v| v == "full"),
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
    /// Where TeX's files come from (`--shelf`, `files`; TeX Live by
    /// default).
    pub files: tex::Files,
    /// What reading `phitex.toml` warned of (unknown keys).
    pub warnings: Vec<String>,
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
        // (the main file, the files it inputs by name, and the classes and
        // packages of those beside them: a file that inputs a LaTeX
        // document is one, a second edition's main file setting a switch
        // before its first's; a thesis class may load fontspec)
        let dir = Path::new(&file)
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let at = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let main = read(&file, true)?;
        let mut texts: Vec<String> = config::braced_inputs(&main)
            .into_iter()
            .filter_map(|name| read(&at(name), true).ok())
            .collect();
        texts.insert(0, main);
        let own: Vec<String> = texts
            .iter()
            .flat_map(|t| config::packages(t))
            .flat_map(|n| [format!("{n}.cls"), format!("{n}.sty")])
            .collect();
        for f in own {
            if let Ok(t) = std::fs::read(at(&f)) {
                texts.push(String::from_utf8_lossy(&t).into_owned());
            }
        }
        let latex = texts
            .iter()
            .any(|t| config::default_engine(t) == "pdflatex");
        let native = texts.iter().find_map(|t| config::native_engine(t));
        match native {
            Some((n, why)) => {
                let e = config::native_program(n, latex);
                tex::engine(e).map_err(|err| {
                    format!(
                        "{file} needs {e} (it {why}): {err}; if it runs under pdflatex too, \
                         name that engine (--engine pdflatex, or `% !TEX program = pdflatex`)"
                    )
                })?
            }
            None => tex::engine(if latex { "pdflatex" } else { "pdftex" })?,
        }
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
    let files = match (o.files, &cfg.files) {
        (Some(f), _) => f,
        (None, Some(f)) => tex::Files::of(f)?,
        (None, None) => tex::Files::TexLive,
    };
    // (TeX Live asked for, none here: said, with the way to Shelf; never
    // a switch to it on our own, one source a run)
    if files == tex::Files::TexLive && !tex::has_texlive() {
        return Err(
            "no TeX Live found on this machine (kpathsea finds no texmf.cnf); \
             PhiTeX can fetch TeX's files from Shelf instead: phitex config files shelf \
             (or --shelf for this run)"
                .to_owned(),
        );
    }
    Ok(Target {
        file,
        engine,
        output_dir,
        copy_pdf,
        // (off unless asked for: the pages' glyph origins it is made
        // from cost the build a little)
        synctex: o.synctex.or(cfg.synctex).unwrap_or(false),
        shell_escape: o.shell_escape || cfg.shell_escape.unwrap_or(false),
        editor: o.editor.clone().or(cfg.editor),
        viewer: cfg.viewer,
        cache_limit: cfg.cache_limit,
        files,
        warnings: cfg.warnings,
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
        edit_diagnostics: o.diagnostics || o.view || o.lsp,
        origins: o.view || o.lsp,
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
    // (the engine's counts and tables: -v)
    if o.verbose > 0 || ui.verbose > 0 {
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
    for f in r
        .out
        .errors
        .iter()
        .filter(|e| !phitex_live::session::transcript_error(e))
    {
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
    let mut s = match Session::new(job, t.files, status.clone(), &mut |n: &str| ui.note(n)) {
        Ok(s) => s,
        Err(e) => {
            spin.done();
            ui.error(&e);
            return crate::EXIT_FAILED;
        }
    };
    // (a package that could not be had, `--shelf`: said)
    {
        let ui = std::sync::Mutex::new(ui.clone());
        s.status().meter().on_unreachable(Box::new(move |why| {
            ui.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .warning(why);
        }));
    }
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
    if let Some(full) = o.report {
        let mut a = crate::report::Report::new(t, o, full);
        a.build(&s, &r, "cold", &r.log_text());
        match a.write(&s, t) {
            Ok(p) => ui.line(&format!(
                "report: {} · file an issue: {}",
                p.display(),
                crate::report::ISSUES
            )),
            Err(e) => ui.warning(&format!("the report: {e}")),
        }
    }
    if !o.lsp && !o.json && !o.quiet {
        crate::upgrade::check(ui);
    }
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

/// The cache's line (dim; with -v, or the cache past 80% of its limit)
/// and `cache` event: after a build, and when a watch ends.
pub fn cache_summary(t: &Target, ui: &Ui) {
    match crate::cache::Cache::open(t.cache_limit) {
        Ok(c) => {
            let full = c.limit > 0 && c.usage() > c.limit / 5 * 4;
            if !ui.quiet && (ui.verbose > 0 || full) {
                let (text, on) = c.summary();
                let st = ui.st;
                if on {
                    ui.line(&format!(
                        "{}{}{}",
                        st.dim(&format!("{text} · run ")),
                        st.cyan(crate::cache::Cache::EMPTY_IT),
                        st.dim(" to empty it")
                    ));
                } else {
                    ui.line(&st.dim(&text));
                }
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
    let mut o = match parse(args) {
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
    if command == Command::News {
        crate::news::print();
        std::process::exit(0);
    }
    if command == Command::Config {
        let (k, v) = o.set.clone().unwrap_or_default();
        match crate::config::set_user(&k, &v) {
            Ok(p) => ui.line(&format!("{k} = {v} in {}", crate::cache::home_relative(&p))),
            Err(e) => {
                ui.error(&e);
                std::process::exit(crate::EXIT_FAILED);
            }
        }
        std::process::exit(0);
    }
    if command == Command::Upgrade {
        std::process::exit(crate::upgrade::upgrade(&ui));
    }
    if !o.lsp && !o.json && !o.quiet {
        crate::news::once(&ui);
        crate::upgrade::begin();
        // (a build's after its result: `build`)
        if command == Command::Watch {
            crate::upgrade::check(&ui);
        }
    }
    // (an editor's language server: its `initialize` first, which says
    // where the watch runs and may name the main file)
    let lsp = if o.lsp && command == Command::Watch {
        if o.json {
            ui.error("--lsp and --json both speak on standard output: one of them");
            std::process::exit(crate::EXIT_FAILED);
        }
        let l = crate::lsp::Lsp::begin(&mut o.file);
        if l.wants_main() {
            o.file = Some(l.main_from_open());
        }
        // (the editor's settings given with `initialize`: over the
        // command line's and phitex.toml's)
        if let Some(n) = l.settings().threads {
            o.threads = Some(n);
        }
        if let Some(f) = l
            .settings()
            .files
            .as_deref()
            .and_then(|f| tex::Files::of(f).ok())
        {
            o.files = Some(f);
        }
        Some(l)
    } else {
        None
    };
    let t = resolve(&o).unwrap_or_else(|e| {
        if let Some(l) = &lsp {
            l.refuse(&e);
        }
        ui.error(&e);
        // (a bare `clean` with no document: what it can do instead)
        if command == Command::Clean && o.file.is_none() {
            let st = ui.st;
            ui.line(&format!(
                "{} {} {} {} {}",
                st.dim("run"),
                st.cyan("phitex clean paper.tex"),
                st.dim("for one document's outputs and cache, or"),
                st.cyan(crate::cache::Cache::EMPTY_IT),
                st.dim("for the whole cache")
            ));
        }
        std::process::exit(crate::EXIT_FAILED);
    });
    for w in &t.warnings {
        ui.warning(w);
    }
    phitex_tex::timeline::lap("cli: the arguments, the target (configuration read)");
    match command {
        Command::Build => std::process::exit(build(&t, &o, &ui)),
        Command::Clean => std::process::exit(clean(&t, &ui)),
        Command::Watch => crate::watch::main(&t, &o, &ui, lsp),
        // (answered before the target)
        Command::News | Command::Upgrade | Command::Config => unreachable!(),
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
