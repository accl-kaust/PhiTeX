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
PhiTeX: a fast TeX compiler with a live preview

`phitex build` makes the PDF as latexmk -pdf does: it runs LaTeX,
BibTeX, biber and makeindex as often as the document needs (at most 5
passes, latexmk's limit), byte for byte the PDF pdflatex makes.
`phitex watch` rebuilds as you edit and shows the pages in your
browser; each PDF is the one a clean build makes. PhiTeX never stops to
ask for input, and never runs an external TeX, BibTeX or \\write18
command.

Exit status: 0 done (warnings allowed); 1 the document has errors (as
pdflatex's 1); 2 PhiTeX could not start: a usage error, a wrong setting,
a file not found, no TeX Live; 3 PhiTeX failed: the engine stopped, a
file could not be written, the network (upgrade, Shelf). Standard error
says which. Under an engine's name (pdflatex, --compat) web2c's 0 and 1.";

/// `--help`'s last lines: where the settings are, and `--help-all`.
fn short_after_help() -> String {
    let l = STYLES.get_literal();
    format!(
        "Settings: phitex.toml (the project's), ~/.config/phitex/phitex.toml (yours; PHITEX_CONFIG: another file):
  {l}phitex config KEY VALUE{l:#}, e.g. {l}phitex config update-check on{l:#}; {l}phitex config --init{l:#} writes one with every setting
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
      synctex = false
      editor = \"code\"   viewer = \"zathura\"   shell-escape = true
                          (yours only, `phitex config KEY VALUE`: a project's are ignored,
                          so that a folder someone sent runs nothing it names)
      cache-limit = \"4GB\"   (0: no cache; or PHITEX_CACHE_LIMIT)
      update-check = true   (yours only: a daily check for a newer release)
      files = \"shelf\"   (TeX's files from Shelf's packs, kept in the cache's shelf/;
                          \"texlive\", kpathsea's, the default; one source a run)
  ~/.config/phitex/phitex.toml holds your own defaults (Windows:
  %APPDATA%\\phitex\\phitex.toml; PHITEX_CONFIG names another file); a
  project's phitex.toml wins over it key by key. `phitex config --init`
  writes a phitex.toml with every setting, commented out (--init=yours: yours).
  A `% !TEX program = …` or `% !TEX root = …` comment in the file is honoured.
  latexmk's rc files are read as latexmk reads them (the system's, yours,
  then `.latexmkrc` or `latexmkrc` in the folder phitex is run in), as data:
  no Perl runs. What helps the build find files applies: TEXINPUTS,
  BIBINPUTS and BSTINPUTS ($ENV{{...}}, ensure_path), and @default_files
  when nothing names the file. Any other setting is said once, not applied;
  --exact-latexmk (or `latexmk = \"exact\"`) applies its engine ($pdf_mode),
  output folder ($out_dir) and options too (docs/latexmkrc.md).
      inputs = [\"../common/tex//\"]   (folders TeX searches first, relative to
                          phitex.toml; a .latexmkrc's TEXINPUTS is over them)

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
            "{}{}\n\
             Copyright 2026 Ammar Seliaman (https://github.com/Flinner, flinner <at> nand <dot> sh)\n\
             AGPL-3.0-only: free software, without any warranty. https://phitex.org\n\
             engine {:016x}, records {}, Shelf schema {} (release {}), editor protocol {}",
            crate::version(),
            if crate::source_build() {
                " (source build)"
            } else {
                ""
            },
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
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Sub>,
    #[command(flatten)]
    global: Global,
}

#[derive(Subcommand, Debug)]
enum Sub {
    /// Build the PDF (its reruns, BibTeX, biber and makeindex included)
    ///
    /// The document is built to its fixpoint as latexmk builds it, the
    /// tools run in process.
    Build {
        /// The main .tex file (default: phitex.toml's `main`)
        #[arg(value_name = "FILE")]
        file: Option<String>,
        #[command(flatten)]
        build: BuildArgs,
    },
    /// Rebuild on every change, the pages live in the browser
    ///
    /// Build, then rebuild whenever a file the document reads changes;
    /// keys on a terminal: r rebuild, q quit, h the rest (with --lsp:
    /// an editor's language client on standard input and output).
    Watch {
        /// The main .tex file (default: phitex.toml's `main`; with --lsp,
        /// the editor's)
        #[arg(value_name = "FILE")]
        file: Option<String>,
        #[command(flatten)]
        build: BuildArgs,
        #[command(flatten)]
        watch: WatchArgs,
    },
    /// The running watches an editor can attach to (--json: as JSON)
    ///
    /// Each `phitex watch` serves its language server on 127.0.0.1 and
    /// says so in a file of its own (docs/spec-editor-protocol.md §8):
    /// its main file, its viewer, its port and token.
    Sessions,
    /// What is new in this version
    News,
    /// Shell completions: phitex completions bash > ~/.local/share/bash-completion/completions/phitex
    ///
    /// The script for SHELL on standard output. bash: into
    /// ~/.local/share/bash-completion/completions/phitex; zsh: a file
    /// _phitex on $fpath; fish: ~/.config/fish/completions/phitex.fish.
    Completions {
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },
    /// Install the latest release
    ///
    /// Its signature checked, for an install made by phitex.org/install.sh
    /// (Windows: install.ps1).
    Upgrade,
    /// Your settings: all of them (phitex config), one (phitex config
    /// synctex), or change one (phitex config update-check on)
    ///
    /// Written into ~/.config/phitex/phitex.toml, its other lines kept.
    Config {
        #[arg(value_name = "KEY")]
        key: Option<String>,
        #[arg(value_name = "VALUE")]
        value: Option<String>,
        /// Take KEY out of your settings (its default holds again)
        #[arg(long, value_name = "KEY", conflicts_with_all = ["key", "value"])]
        unset: Option<String>,
        /// Write a phitex.toml with every setting, commented out: here (the
        /// project's) or yours
        #[arg(
            long,
            value_name = "WHERE",
            num_args = 0..=1,
            default_missing_value = "here",
            value_parser = ["here", "yours"],
            conflicts_with_all = ["key", "value", "unset"]
        )]
        init: Option<String>,
    },
    /// Remove a build's files (--all: the whole cache; no file: the
    /// cache's status)
    Clean {
        /// The whole cache, every document's (no file then)
        #[arg(long, conflicts_with = "file")]
        all: bool,
        /// Say what would be removed, remove nothing
        #[arg(short = 'n', long)]
        dry_run: bool,
        /// Where the build's outputs are (TeX's -output-directory)
        #[arg(
            short = 'o',
            long = "output-dir",
            alias = "output-directory",
            alias = "out-dir",
            value_name = "DIR"
        )]
        output_dir: Option<String>,
        /// The main .tex file (default: phitex.toml's `main`)
        #[arg(value_name = "FILE")]
        file: Option<String>,
    },
}

/// The options of every command: how much is said, how, and whether
/// questions are asked.
#[derive(Args, Debug, Default)]
struct Global {
    /// More detail: each pass, each warning, the cache (-vv: the engine's counters)
    ///
    /// Each pass over the document and why it ran, each warning's first
    /// lines, a failure's whole message, the cache's use; -vv adds the
    /// engine's own counters and tables
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,
    /// Only problems and the result
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    quiet: bool,
    /// Yes to every question: upgrade's, and watch's whether to write
    /// the PDF on quitting
    #[arg(short = 'y', long = "no-confirm", visible_alias = "yes", global = true)]
    no_confirm: bool,
    /// The build's events on standard output, one JSON object a line
    /// (nothing else is printed)
    #[arg(long, global = true, hide_short_help = true)]
    json: bool,
    /// Print help (--help-all: every option)
    #[arg(short = 'h', long = "help", action = clap::ArgAction::HelpShort, global = true)]
    help: Option<bool>,
    /// Print every option, with the details
    #[arg(long = "help-all", action = clap::ArgAction::HelpLong, global = true, hide_short_help = true)]
    help_all: Option<bool>,
    /// When to use colours: auto (on a terminal, unless NO_COLOR), always, never
    #[arg(long, value_name = "WHEN", value_parser = ["auto", "always", "never"], global = true, hide_short_help = true)]
    color: Option<String>,
}

/// What to build and how: `build`'s and `watch`'s options.
#[derive(Args, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
struct BuildArgs {
    /// The exact PDF file on every edit: a watch writes it each time
    /// (else an edit appends an update section, and the exact file is
    /// written once the editor is idle); a build always writes it, so
    /// there it changes nothing (scripts give it to build and watch alike)
    #[arg(long, alias = "exact-pdf", hide_short_help = true)]
    exact: bool,
    /// Where the outputs go (TeX's -output-directory)
    #[arg(
        short = 'o',
        long = "output-dir",
        alias = "output-directory",
        alias = "out-dir",
        value_name = "DIR"
    )]
    output_dir: Option<String>,
    /// pdflatex (default), xelatex, latex, pdftex, …
    ///
    /// pdflatex (default for LaTeX), latex, xelatex, pdftex (default
    /// otherwise), xetex, etex, tex (lualatex is not supported yet)
    #[arg(long, alias = "program", value_name = "NAME")]
    engine: Option<String>,
    /// dvi or pdf (xdv: XeTeX's XDV file, not made a PDF)
    #[arg(long, value_name = "FORMAT", value_parser = ["dvi", "pdf", "xdv"], hide_short_help = true)]
    output_format: Option<String>,
    /// The file is the whole job: no format (TeX's -ini)
    #[arg(long, hide_short_help = true)]
    ini: bool,
    /// With --ini: e-TeX's extended mode
    #[arg(long, hide_short_help = true)]
    etex: bool,
    /// Threads (default: 1); the output is the same at any count
    ///
    /// The engine's threads (default: PHITEX_THREADS, else 1); the output
    /// is the same at any count
    #[arg(short = 'j', long = "jobs", alias = "threads", value_name = "N")]
    jobs: Option<usize>,
    /// Whether the engine runs steps ahead of their inputs: on those an
    /// earlier run predicts (the default), never, or on any (not usable
    /// yet). The output is the same either way.
    #[arg(long = "spec", value_name = "WHEN", value_parser = ["off", "predicted", "all"], hide_short_help = true)]
    spec: Option<String>,
    /// A long piece's readers do not wait for it on the fields an earlier
    /// build learned it defines last before them (they do by default: kept
    /// beside the format). The output is the same either way.
    #[arg(long = "no-wait-long", hide = true)]
    no_wait_long: bool,
    /// The document nodes' records read and stored (off by default; kept
    /// per document in the cache). The output is the same either way.
    #[arg(long = "node-records", hide = true)]
    node_records: bool,
    /// \pdfshellescape 1 instead of TeX Live's restricted default (where
    /// \write18 may name bibtex and makeindex): biber is allowed too. The
    /// tools run in process; no other command runs either way
    #[arg(long, hide_short_help = true)]
    shell_escape: bool,
    /// Apply all of latexmk's rc files that PhiTeX reads: their engine
    /// ($pdf_mode, $pdflatex), output folder ($out_dir), SyncTeX and
    /// -shell-escape, not only their search paths (`latexmk = "exact"` in
    /// phitex.toml, or `phitex config latexmk exact`; -shell-escape only
    /// from this flag)
    #[arg(long, hide_short_help = true)]
    exact_latexmk: bool,
    /// Copy the PDF after each successful build into DIR (default: where
    /// phitex was run)
    #[arg(long, value_name = "DIR", num_args = 0..=1, require_equals = true, default_missing_value = "", hide_short_help = true)]
    copy_pdf: Option<String>,
    #[arg(long, hide = true)]
    no_copy_pdf: bool,
    /// Write <job>.synctex.gz, for a PDF viewer's jumps between source
    /// and pages (`synctex = true` in `phitex.toml`)
    #[arg(long, overrides_with = "no_synctex", hide_short_help = true)]
    synctex: bool,
    /// Write <job>.phitex-report.txt beside the outputs, for a bug report
    /// (no text of the document; `--report=full` adds the .log and the
    /// project's sources)
    #[arg(long, value_name = "full", num_args = 0..=1, require_equals = true, default_missing_value = "", value_parser = ["", "full"], hide_short_help = true)]
    report: Option<String>,
    /// No <job>.synctex.gz (the default)
    #[arg(long, overrides_with = "synctex", hide_short_help = true)]
    no_synctex: bool,
    /// TeX's files from Shelf's packs (fetched once, kept in the cache's
    /// `shelf/`; v1.shelf.phitex.org the server, PHITEX_SHELF_URL another) instead of TeX Live's
    /// (`files = "shelf"` in `phitex.toml`)
    #[arg(long, hide_short_help = true)]
    shelf: bool,
}

/// `watch`'s own options: the viewer, the editor, what is written.
#[derive(Args, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
struct WatchArgs {
    /// watch: open the viewer
    ///
    /// watch: open the viewer in the browser (or start phitex.toml's
    /// `viewer`, zathura or the like, on the output)
    #[arg(long)]
    open: bool,
    /// watch: no live viewer (the PDF and its files are then written, as by --write)
    #[arg(long, hide_short_help = true)]
    no_view: bool,
    /// watch: write the PDF (with its .synctex.gz), the log and the aux
    /// files into the folder after each build, as build does; without it
    /// a watch writes nothing there (the pages are in the viewer;
    /// `watch-write = true` in phitex.toml always writes)
    #[arg(long, hide_short_help = true)]
    write: bool,
    /// watch: the diagnostics at their places after every edit too (the
    /// viewer asks for them; else an edit reports the engine's own only)
    #[arg(long, hide_short_help = true)]
    diagnostics: bool,
    /// watch: the time without an edit after which the exact PDF file is
    /// written over the updated one (default 300)
    #[arg(long, value_name = "MS", hide_short_help = true)]
    idle: Option<u64>,
    /// watch: also write the .log file, each time the watch goes idle
    /// (beside the exact PDF). Without it a watch writes no .log; build
    /// always writes one
    #[arg(long, hide_short_help = true)]
    log: bool,
    /// watch: be an editor's language server on standard input and
    /// output (LSP): its unsaved buffers built as you type, the problems
    /// shown in it, a double-click on a page opening it there, its cursor
    /// shown on the page (`phitex/cursor`); nothing else on standard output
    #[arg(long, hide_short_help = true)]
    lsp: bool,
    /// watch: the viewer's port (default: a free one)
    #[arg(long, value_name = "N", hide_short_help = true)]
    port: Option<u16>,
    /// watch: where a double-click on a page opens its source (code,
    /// emacsclient, nvim, … or a command with {file} {line} {col}; else
    /// PHITEX_EDITOR, VISUAL, EDITOR)
    #[arg(long, value_name = "CMD", hide_short_help = true)]
    editor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Build,
    Watch,
    Clean,
    News,
    Upgrade,
    Config,
    Sessions,
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
    /// `--no-confirm`: every question answered yes, none asked.
    pub no_confirm: bool,
    pub json: bool,
    pub shell_escape: bool,
    /// `--exact-latexmk`.
    pub exact_latexmk: bool,
    pub open: bool,
    pub view: bool,
    /// watch: the files written into the folder (`--write`).
    pub write: bool,
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
    /// `clean --dry-run`: what would be removed said, nothing removed.
    pub dry_run: bool,
    /// `--shelf`, or the editor's `files` setting (none: `phitex.toml`'s,
    /// else TeX Live).
    pub files: Option<tex::Files>,
    /// `config`'s key and value (none: list them; a key alone: show it).
    pub set: Option<(Option<String>, Option<String>)>,
    /// `--report` (true: `=full`).
    pub report: Option<bool>,
    /// `--color`: always (true), never (false), else by the stream.
    pub color: Option<bool>,
    /// `config --unset KEY`.
    pub unset: Option<String>,
    /// `config --init`: `here` (a project's phitex.toml) or `yours`.
    pub init: Option<String>,
}

impl Cli {
    fn into_options(self) -> Options {
        let g = self.global;
        let mut o = Options {
            verbose: g.verbose,
            quiet: g.quiet,
            no_confirm: g.no_confirm,
            json: g.json,
            color: g.color.as_deref().and_then(|c| match c {
                "always" => Some(true),
                "never" => Some(false),
                _ => None,
            }),
            view: true,
            wait_long: true,
            idle: Duration::from_millis(300),
            ..Options::default()
        };
        match self.command {
            Some(Sub::Build { file, build }) => {
                o.command = Some(Command::Build);
                o.file = file;
                build.apply(&mut o);
            }
            Some(Sub::Watch { file, build, watch }) => {
                o.command = Some(Command::Watch);
                o.file = file;
                build.apply(&mut o);
                watch.apply(&mut o);
            }
            Some(Sub::Clean {
                all,
                dry_run,
                output_dir,
                file,
            }) => {
                o.command = Some(Command::Clean);
                o.file = file;
                o.all = all;
                o.dry_run = dry_run;
                o.output_dir = output_dir;
            }
            Some(Sub::News) => o.command = Some(Command::News),
            Some(Sub::Sessions) => o.command = Some(Command::Sessions),
            Some(Sub::Completions { shell }) => {
                let mut cmd = <Cli as clap::CommandFactory>::command();
                clap_complete::generate(shell, &mut cmd, "phitex", &mut std::io::stdout());
                std::process::exit(0);
            }
            Some(Sub::Upgrade) => o.command = Some(Command::Upgrade),
            Some(Sub::Config {
                key,
                value,
                unset,
                init,
            }) => {
                o.command = Some(Command::Config);
                o.set = Some((key, value));
                o.unset = unset;
                o.init = init;
            }
            None => {}
        }
        o
    }
}

impl BuildArgs {
    fn apply(self, o: &mut Options) {
        let b = self;
        o.exact_file = b.exact;
        o.engine = b.engine;
        o.output = b.output_format.as_deref().map(|f| match f {
            "dvi" => Output::Dvi,
            "xdv" => Output::XdvFile,
            _ => Output::Pdf,
        });
        o.ini = b.ini;
        o.etex = b.etex;
        o.output_dir = b.output_dir;
        o.threads = b.jobs;
        o.spec = b.spec;
        o.wait_long = !b.no_wait_long;
        o.node_records = b.node_records;
        o.shell_escape = b.shell_escape;
        o.exact_latexmk = b.exact_latexmk;
        o.copy_pdf = if b.no_copy_pdf {
            Some(config::CopyPdf::Off)
        } else {
            b.copy_pdf.map(|d| {
                if d.is_empty() {
                    config::CopyPdf::Invocation
                } else {
                    config::CopyPdf::Dir(d)
                }
            })
        };
        o.synctex = if b.synctex {
            Some(true)
        } else {
            b.no_synctex.then_some(false)
        };
        o.files = b.shelf.then_some(tex::Files::Shelf);
        o.report = b.report.map(|v| v == "full");
    }
}

impl WatchArgs {
    fn apply(self, o: &mut Options) {
        let w = self;
        o.open = w.open;
        o.view = !w.no_view;
        o.write = w.write;
        o.diagnostics = w.diagnostics;
        o.idle = Duration::from_millis(w.idle.unwrap_or(300));
        o.log = w.log;
        o.port = w.port;
        o.lsp = w.lsp;
        o.editor = w.editor;
    }
}

/// The command line's options (`args` after the program's name).
fn parse(args: &[String]) -> Result<Options, clap::Error> {
    use clap::{CommandFactory as _, FromArgMatches as _};
    let line = std::iter::once("phitex").chain(args.iter().map(String::as_str));
    // (the help's colours as the rest's: NO_COLOR over CLICOLOR_FORCE)
    let mut cmd = Cli::command();
    if std::env::var_os("NO_COLOR").is_some_and(|x| !x.is_empty()) {
        cmd = cmd.color(ColorChoice::Never);
    }
    let m = cmd.try_get_matches_from(line)?;
    Cli::from_arg_matches(&m).map(Cli::into_options)
}

/// clap's long help (`--help-all`) without the blank lines between the
/// options of a section (clap puts one after every option once any has a
/// long help): a blank line kept only before a line that is not indented
/// (a section's header, the text after the options).
fn compact_help(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    for (k, l) in lines.iter().enumerate() {
        if crate::ui::plain(l).trim().is_empty() {
            let next = lines[k + 1..]
                .iter()
                .map(|n| crate::ui::plain(n))
                .find(|n| !n.trim().is_empty());
            let prev = k.checked_sub(1).map(|j| crate::ui::plain(lines[j]));
            let indented = |t: &str| t.starts_with("  ");
            if next.as_deref().is_some_and(indented) && prev.as_deref().is_some_and(indented) {
                continue;
            }
        }
        out.push_str(l);
        out.push('\n');
    }
    out
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
    /// Whether `synctex` was asked for or turned off (`--synctex`,
    /// `--no-synctex`, `synctex =`): else a watch that writes the PDF
    /// writes its `.synctex.gz` with it.
    pub synctex_said: bool,
    /// watch: write the files into the folder (`watch-write`).
    pub watch_write: bool,
    /// `\write18` runs commands.
    pub shell_escape: bool,
    /// watch: the editor a double-click opens (`--editor`, `phitex.toml`).
    pub editor: Option<String>,
    /// watch: a program `--open` starts on the output instead of the
    /// browser (`viewer` in `phitex.toml`).
    pub viewer: Option<String>,
    /// The cache's limit (`cache-limit`; none: the default).
    pub cache_limit: Option<u64>,
    /// The engine's threads (`threads` in the user's file; none or 0: the default, one
    /// thread), under `-j`.
    pub threads: Option<usize>,
    /// Where TeX's files come from (`--shelf`, `files`; TeX Live by
    /// default).
    pub files: tex::Files,
    /// What reading `phitex.toml` and `.latexmkrc` warned of (unknown
    /// keys, lines not applied).
    pub warnings: Vec<String>,
    /// The search paths a `.latexmkrc` set (the job's `inputs`).
    pub inputs: Vec<(String, String)>,
    /// The settings' files read, and those looked for and not there
    /// (absolute): `phitex.toml`, the user's, the `.latexmkrc` files and
    /// those they `do`. A watch resolves its target again when one
    /// changes.
    pub config_files: Vec<PathBuf>,
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

/// The directory phitex was started in (a watch resolves its target
/// again after it entered the build's): where latexmk would be run.
static STARTED_IN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// [`resolve`] again, from the directory phitex was started in (where
/// `o`'s file is named from, and latexmk's rc files are read), as the
/// first time: the target (its job made), and the directory its build
/// runs in. The
/// current directory is left as it was (a watch's session runs on until
/// the next begins there).
pub(crate) fn resolve_again(o: &Options) -> Result<(Target, PathBuf), String> {
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    if let Some(started) = STARTED_IN.get() {
        std::env::set_current_dir(started)
            .map_err(|e| format!("can't enter {}: {e}", started.display()))?;
    }
    // (its job made there, where its file is named from)
    let t = resolve(o).and_then(|t| job(&t, o).map(|_| t));
    let dir = std::env::current_dir().map_err(|e| e.to_string());
    std::env::set_current_dir(&here).map_err(|e| format!("can't enter {}: {e}", here.display()))?;
    Ok((t?, dir?))
}

pub(crate) fn resolve(o: &Options) -> Result<Target, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let started = STARTED_IN.get_or_init(|| cwd.clone()).clone();
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
    // (the project's settings over the user's, key by key; but anything that
    // runs is the user's to allow: a project someone sent can't ask for it)
    // (`inputs`: the project's folders, relative to its phitex.toml, else
    // yours, relative to your file's folder)
    let toml_inputs = match (&cfg.inputs, &user.inputs) {
        (Some(i), _) => Some((i.clone(), root.clone())),
        (None, Some(i)) => config::user_file()
            .and_then(|f| f.parent().map(Path::to_path_buf))
            .map(|d| (i.clone(), d)),
        (None, None) => None,
    };
    let cfg = config::without_permissions(cfg, &root).over(user);
    // (latexmk's rc files, as latexmk run here reads them: the system's,
    // the user's, then this directory's, never a parent's; by default only
    // what helps the build find files applies, the rest is said;
    // `--exact-latexmk` or `latexmk = "exact"` applies the rest too.
    // phitex.toml's `inputs` come first on TEXINPUTS as the rc files see
    // it: a .latexmkrc that sets TEXINPUTS outright is over them)
    let exact = o.exact_latexmk || cfg.latexmk.as_deref() == Some("exact");
    let start = toml_inputs.map(|(dirs, base)| {
        let base = base.to_string_lossy();
        let mut v: Vec<String> = dirs
            .iter()
            .map(|d| phitex_live::latexmkrc::absolute_elements(d, &base))
            .collect();
        v.push(std::env::var("TEXINPUTS").unwrap_or_default());
        v.join(":")
    });
    let rc = config::latexmkrc(&started, start.as_deref());
    // (what a change of rereads: the rc files read, those latexmk run here
    // would read if they were there, phitex.toml, the user's)
    let mut config_files: Vec<PathBuf> = rc.read.iter().map(|f| started.join(f)).collect();
    config_files.extend([".latexmkrc", "latexmkrc"].map(|f| started.join(f)));
    config_files.push(if configured {
        root.join(config::FILE)
    } else {
        started.join(config::FILE)
    });
    config_files.extend(config::user_file());
    config_files.sort();
    config_files.dedup();
    let mut inputs = rc.vars.clone();
    if let Some(start) = start
        && rc.var("TEXINPUTS").is_none()
    {
        inputs.push(("TEXINPUTS".into(), start));
    }
    let applied = exact.then(|| rc.exact());
    let copy_pdf = match (&o.copy_pdf, &cfg.copy_pdf) {
        (Some(c), _) => copy_dir(c, &cwd, &cwd),
        (None, Some(c)) => copy_dir(c, &cwd, &root),
        (None, None) => None,
    };
    let mut rc_warnings: Vec<String> = rc.notes.iter().map(ToString::to_string).collect();
    match &applied {
        None => rc_warnings.extend(rc.not_applied(Some("--exact-latexmk"))),
        Some(a) => rc_warnings.extend(a.notes.iter().map(ToString::to_string)),
    }
    // (with --exact-latexmk: a .latexmkrc's engine, output folder, SyncTeX
    // and -shell-escape, each under the command line's own option)
    let rc_engine = applied.as_ref().and_then(|a| a.engine.clone());
    let rc_synctex = applied.as_ref().and_then(|a| a.synctex);
    let rc_out_dir = applied.as_ref().and_then(|a| {
        let (d, at) = a.out_dir.as_ref()?;
        // (a project's folder may be anyone's: outside it, only when you
        // asked on the command line)
        let inside = config::inside(&started, d);
        if !inside && !o.exact_latexmk {
            rc_warnings.push(format!(
                "{at}: $out_dir not applied: it is outside the folder; --exact-latexmk on \
                 the command line applies it"
            ));
            return None;
        }
        Some(started.join(d).to_string_lossy().into_owned())
    });
    let rc_shell_escape = match applied.as_ref().and_then(|a| a.shell_escape.clone()) {
        Some((true, at)) if !o.exact_latexmk => {
            rc_warnings.push(format!(
                "{at}: -shell-escape not applied: a .latexmkrc may be anyone's; \
                 --exact-latexmk on the command line (or --shell-escape) applies it"
            ));
            false
        }
        Some((on, _)) => on,
        None => false,
    };
    // (`@default_files` names the main file when nothing else does: a
    // phitex.toml's `main` is a file named, as one on the command line)
    let from_rc = rc
        .default_files
        .as_ref()
        .and_then(|(files, at)| {
            let first = files.first()?;
            if files.len() > 1 {
                rc_warnings.push(format!(
                    "{at}: @default_files names {} files: phitex builds the first, {first} \
                     (latexmk builds each)",
                    files.len()
                ));
            }
            // (latexmk's are the directory it runs in's)
            let p = started.join(first);
            Some(if started == cwd {
                first.clone()
            } else {
                p.to_string_lossy().into_owned()
            })
        })
        .filter(|_| o.file.is_none() && cfg.main.is_none());
    let mut file = o
        .file
        .clone()
        .or_else(|| cfg.main.clone())
        .or(from_rc)
        .ok_or("no file given, and no phitex.toml names one (`main = \"paper.tex\"`)".to_owned())?;
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
    // (where the engine was named, for an error that says which to fix)
    let named = o
        .engine
        .clone()
        .map(|e| (e, "--engine".to_owned()))
        .or(rc_engine.map(|e| (e, "the .latexmkrc's $pdf_mode".to_owned())))
        .or(cfg.engine.map(|e| {
            (
                e,
                "engine in phitex.toml (yours or the project's)".to_owned(),
            )
        }))
        .or(program.map(|e| (e, format!("{file}'s % !TEX program"))));
    let mut engine = if let Some((e, at)) = named {
        tex::engine(&e).map_err(|err| format!("{err} (from {at})"))?
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
    let mut output_dir = o.output_dir.clone().or(rc_out_dir).or(cfg.output_dir);
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
    // (`clean` reads no TeX file: it needs none)
    if files == tex::Files::TexLive && o.command != Some(Command::Clean) && !tex::has_texlive() {
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
        synctex: o.synctex.or(rc_synctex).or(cfg.synctex).unwrap_or(false),
        synctex_said: o.synctex.or(rc_synctex).or(cfg.synctex).is_some(),
        watch_write: cfg.watch_write.unwrap_or(false),
        shell_escape: o.shell_escape || rc_shell_escape || cfg.shell_escape.unwrap_or(false),
        editor: o.editor.clone().or(cfg.editor),
        viewer: cfg.viewer,
        cache_limit: cfg.cache_limit,
        threads: cfg.threads.filter(|&n| n > 0),
        files,
        warnings: [cfg.warnings, rc_warnings].concat(),
        inputs,
        config_files,
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
        threads: o.threads.or(t.threads),
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
        // (every watch: editors may attach to any, `attach.rs`, and its
        // terminal says an edit's errors at their places too)
        edit_diagnostics: o.diagnostics || o.view || o.lsp || o.command == Some(Command::Watch),
        origins: o.view || o.lsp,
        inputs: t.inputs.clone(),
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
    let verbose = o.verbose.max(ui.verbose);
    // (-v: each pass over the document, in a reader's words; -vv: the
    // engine's own counters and tables too)
    if verbose > 0 {
        for (k, run) in r.runs.iter().enumerate() {
            let engine = if verbose > 1 {
                format!(
                    ", {} nodes{}",
                    run.commands_run,
                    run.parallel().map(|p| format!(", {p}")).unwrap_or_default()
                )
            } else {
                String::new()
            };
            ui.line(&st.dim(&format!(
                "  pass {}: {} ({}){engine}",
                k + 1,
                ms(run.wall),
                crate::ui::said_why(&run.why),
            )));
        }
    }
    if verbose > 1 {
        for n in &r.notes {
            ui.line(&st.dim(&format!("  {n}")));
        }
    }
    if let (Some(d), true) = (&r.internal, verbose > 1) {
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
            return crate::EXIT_USAGE;
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
            return crate::EXIT_INTERNAL;
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
            return crate::EXIT_INTERNAL;
        }
    };
    status.set("writing the outputs");
    phitex_tex::timeline::lap("cli: the build returned");
    let main = match s.write(&r, true) {
        Ok(w) => w.main,
        Err(e) => {
            spin.done();
            ui.error(&e);
            return crate::EXIT_INTERNAL;
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
        a.build(&s, &r, "clean build", &r.log_text());
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
    if r.out.errors.is_empty() {
        crate::star::built(Path::new(&t.file), ui, o.lsp);
    }
    // (the engine's own failure, not the document's errors: 3)
    let code = if r
        .out
        .errors
        .iter()
        .any(|e| !phitex_live::session::transcript_error(e))
    {
        crate::EXIT_INTERNAL
    } else {
        i32::from(!r.out.errors.is_empty())
    };
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

/// `phitex clean`: the document's outputs and its part of the cache;
/// `dry`: said, not removed.
fn clean(t: &Target, ui: &Ui, dry: bool) -> i32 {
    let dir = t
        .output_dir
        .as_ref()
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let there: Vec<PathBuf> = CLEAN
        .iter()
        .map(|ext| dir.join(format!("{}.{ext}", t.job())))
        .filter(|p| p.is_file())
        .collect();
    let cache = crate::cache::Cache::open(t.cache_limit);
    let st = ui.st;
    if dry {
        for p in &there {
            ui.line(&format!("{} {}", st.dim("would remove"), p.display()));
        }
        let held = cache
            .as_ref()
            .map_or(0, |c| c.doc_bytes(Path::new(&t.file)));
        ui.line(&format!(
            "{} {} of cache {}",
            st.dim("would remove"),
            crate::cache::size(held),
            st.dim(&format!("({}'s)", t.file))
        ));
        ui.line(&format!(
            "{} {} {}",
            st.dim("nothing removed: run"),
            st.cyan(&format!("phitex clean {}", t.file)),
            st.dim("to remove them")
        ));
        ui.event(&json!({"event": "clean", "dry_run": true, "files": there.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(), "cache_bytes": held}));
        return 0;
    }
    let n = there
        .iter()
        .filter(|p| std::fs::remove_file(p).is_ok())
        .count();
    let freed = match cache {
        Ok(c) => c.clear_doc(Path::new(&t.file)).unwrap_or_else(|e| {
            ui.warning(&format!("the document's cache: {e}"));
            0
        }),
        Err(e) => {
            ui.warning(&e);
            0
        }
    };
    ui.line(&st.green(&format!(
        "removed {n} file{} and {} of cache",
        if n == 1 { "" } else { "s" },
        crate::cache::size(freed)
    )));
    ui.event(&json!({"event": "clean", "removed": n, "cache_bytes": freed}));
    0
}

/// A bare `phitex clean` (no file, no `--all`): nothing removed; the
/// cache's status and what each form of the command removes, `main` the
/// project's main file where its phitex.toml names one.
fn clean_status(ui: &Ui, main: Option<&str>) -> ! {
    let st = ui.st;
    match crate::cache::Cache::open(None) {
        Ok(c) => {
            ui.line(&c.summary().0);
            ui.event(&json!({"event": "cache", "dir": c.dir.display().to_string(), "bytes": c.usage(), "limit": c.limit}));
        }
        Err(e) => ui.warning(&e),
    }
    let file = main.unwrap_or("paper.tex");
    let rows = [
        (
            format!("phitex clean {file}"),
            "removes its outputs and its cache",
        ),
        (
            format!("phitex clean -n {file}"),
            "says what that would remove, removes nothing",
        ),
        (
            crate::cache::Cache::EMPTY_IT.to_owned(),
            "empties the whole cache (-n: says how much)",
        ),
    ];
    let w = rows
        .iter()
        .map(|(c, _)| c.chars().count())
        .max()
        .unwrap_or(0);
    for (cmd, what) in &rows {
        ui.line(&format!(
            "  {}  {}",
            st.cyan(&crate::ui::pad(cmd, w, false)),
            st.dim(what)
        ));
    }
    std::process::exit(0);
}

/// `phitex clean --all`: the whole cache, every document's (`dry`: said,
/// not removed).
fn clean_all(ui: &Ui, dry: bool) -> ! {
    let c = crate::cache::Cache::open(None).unwrap_or_else(|e| {
        ui.error(&e);
        std::process::exit(crate::EXIT_INTERNAL);
    });
    let dir = crate::cache::home_relative(&c.dir);
    if dry {
        let st = ui.st;
        ui.line(&format!(
            "{} {} of cache ({dir})",
            st.dim("would remove"),
            crate::cache::size(c.usage())
        ));
        ui.line(&format!(
            "{} {} {}",
            st.dim("nothing removed: run"),
            st.cyan(crate::cache::Cache::EMPTY_IT),
            st.dim("to empty it")
        ));
        ui.event(&json!({"event": "clean", "dry_run": true, "cache_bytes": c.usage()}));
        std::process::exit(0);
    }
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
            std::process::exit(crate::EXIT_INTERNAL);
        }
    }
    std::process::exit(0);
}

/// Run the modern command line with `args`.
pub fn main(args: &[String]) -> ! {
    // (clap's own exit status for a usage error is 2: pdflatex's is 1)
    let mut o = match parse(args) {
        Ok(o) => o,
        // (`--help-all`: clap's long help without its blank line between
        // every two options)
        Err(e)
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                && args.iter().any(|a| a == "--help-all") =>
        {
            use std::io::IsTerminal as _;
            let styled = e.render();
            let color = std::io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none_or(|x| x.is_empty());
            let text = if color {
                styled.ansi().to_string()
            } else {
                styled.to_string()
            };
            print!("{}", compact_help(&text));
            std::process::exit(0);
        }
        Err(e) => {
            let _ = e.print();
            std::process::exit(if e.use_stderr() { crate::EXIT_USAGE } else { 0 });
        }
    };
    crate::ui::set_color(o.color);
    let ui = Ui::new(Style::detect(), o.verbose, o.quiet, o.json);
    crate::ui::capture_panics(o.verbose > 0);
    if o.all {
        clean_all(&ui, o.dry_run);
    }
    // (`PHITEX_TEX_TIME`: a cold build's phases from the process's start)
    if matches!(o.command, Some(Command::Build))
        && let Some(&t0) = crate::START.get()
    {
        phitex_tex::timeline::begin_at(t0);
    }
    let Some(command) = o.command else {
        ui.error(
            "no command given: build, watch, clean, config, news or upgrade (`phitex --help`)",
        );
        std::process::exit(crate::EXIT_USAGE);
    };
    if command == Command::News {
        crate::news::print(o.json);
        std::process::exit(0);
    }
    if command == Command::Sessions {
        std::process::exit(crate::attach::sessions(&ui, o.json));
    }
    if command == Command::Config {
        if let Some(w) = &o.init {
            let yours = w == "yours";
            let path = if yours {
                crate::config::user_file()
            } else {
                Some(PathBuf::from(crate::config::FILE))
            };
            let Some(path) = path else {
                ui.error("no home directory for your phitex.toml (or set PHITEX_CONFIG to a file)");
                std::process::exit(crate::EXIT_USAGE);
            };
            match crate::config::init(&path, yours) {
                Ok(()) => {
                    let st = ui.st;
                    ui.line(&format!(
                        "{} {}",
                        st.green(&format!("wrote {}:", crate::cache::home_relative(&path))),
                        st.dim("every setting, commented out; uncomment a line to set it")
                    ));
                    ui.event(&json!({"event": "config", "init": path.display().to_string()}));
                    std::process::exit(0);
                }
                Err(e) => {
                    ui.error(&e);
                    std::process::exit(crate::EXIT_USAGE);
                }
            }
        }
        if let Some(k) = &o.unset {
            match crate::config::unset_user(k) {
                Ok(p) => {
                    ui.line(&format!(
                        "{k} taken out of {}",
                        crate::cache::home_relative(&p)
                    ));
                    ui.event(&json!({"event": "config", "key": k, "value": null, "file": p.display().to_string()}));
                    std::process::exit(0);
                }
                Err(e) => {
                    ui.error(&e);
                    std::process::exit(crate::EXIT_USAGE);
                }
            }
        }
        let (k, v) = o.set.clone().unwrap_or_default();
        let (Some(k), Some(v)) = (k.clone(), v) else {
            // (the listing is the command's answer: standard output, as
            // `git config`'s; `--json` one object)
            let cwd = std::env::current_dir().unwrap_or_default();
            let shown = if o.json {
                crate::config::listing_json(k.as_deref(), &cwd).map(|v| vec![v.to_string()])
            } else {
                crate::config::listing(k.as_deref(), &cwd, Style::stdout())
            };
            match shown {
                Ok(lines) => lines.iter().for_each(|l| println!("{l}")),
                Err(e) => {
                    ui.error(&e);
                    std::process::exit(crate::EXIT_USAGE);
                }
            }
            std::process::exit(0);
        };
        match crate::config::set_user(&k, &v) {
            Ok(p) => {
                ui.line(&format!("{k} = {v} in {}", crate::cache::home_relative(&p)));
                ui.event(&json!({"event": "config", "key": k, "value": v, "file": p.display().to_string()}));
            }
            Err(e) => {
                ui.error(&e);
                std::process::exit(crate::EXIT_USAGE);
            }
        }
        std::process::exit(0);
    }
    if command == Command::Upgrade {
        std::process::exit(crate::upgrade::upgrade(&ui, o.no_confirm));
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
            std::process::exit(crate::EXIT_USAGE);
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
    // (a bare `clean`: nothing removed, the cache's status and the
    // commands, the project's main file named where phitex.toml has one)
    if command == Command::Clean && o.file.is_none() {
        let main = resolve(&o).ok().map(|t| t.file);
        clean_status(&ui, main.as_deref());
    }
    let t = resolve(&o).unwrap_or_else(|e| {
        if let Some(l) = &lsp {
            l.refuse(&e);
        }
        ui.error(&e);
        std::process::exit(crate::EXIT_USAGE);
    });
    for w in &t.warnings {
        ui.warning(w);
    }
    phitex_tex::timeline::lap("cli: the arguments, the target (configuration read)");
    match command {
        Command::Build => std::process::exit(build(&t, &o, &ui)),
        Command::Clean => std::process::exit(clean(&t, &ui, o.dry_run)),
        Command::Watch => crate::watch::main(&t, &o, &ui, lsp),
        // (answered before the target)
        Command::News | Command::Upgrade | Command::Config | Command::Sessions => unreachable!(),
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

    /// Each option where it means something: a build's after `build` or
    /// `watch` (before or after the file), the watch's after `watch`
    /// alone; one where it means nothing is refused, not ignored. -v,
    /// -q, -y, --json and --color anywhere.
    #[test]
    fn options_where_they_mean_something() {
        for line in [
            ["watch", "-o", "out", "-j", "2", "f.tex"],
            ["watch", "f.tex", "-o", "out", "-j", "2"],
        ] {
            let o = parse(&v(&line)).unwrap();
            assert_eq!(o.command, Some(Command::Watch), "{line:?}");
            assert_eq!(o.output_dir.as_deref(), Some("out"), "{line:?}");
            assert_eq!(o.threads, Some(2), "{line:?}");
            assert_eq!(o.file.as_deref(), Some("f.tex"), "{line:?}");
        }
        // (the editors' lines: VS Code's, Neovim's and Emacs's, the file after)
        for line in [
            &["watch", "paper.tex", "--lsp"][..],
            &["watch", "--lsp"],
            &["watch", "--lsp", "paper.tex"],
        ] {
            let o = parse(&v(line)).unwrap();
            assert!(o.lsp, "{line:?}");
            assert_eq!(o.file.is_some(), line.contains(&"paper.tex"), "{line:?}");
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
        assert!(parse(&v(&["-y", "upgrade"])).unwrap().no_confirm);
        assert!(parse(&v(&["news", "--json"])).unwrap().json);
        for meaningless in [
            &["build", "--lsp", "x.tex"][..],
            &["build", "--open", "x.tex"],
            &["build", "--port", "1", "x.tex"],
            &["news", "--shelf"],
            &["news", "-j", "9"],
            &["config", "--engine", "tex"],
            &["upgrade", "-o", "out"],
            &["clean", "--all", "x.tex"],
            &["-o", "out", "build", "x.tex"],
            &["build", "-q", "-v", "x.tex"],
        ] {
            assert!(parse(&v(meaningless)).is_err(), "{meaningless:?} accepted");
        }
    }
}
