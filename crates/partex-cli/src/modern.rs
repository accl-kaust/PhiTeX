//! The modern command line: `phitex build`, `phitex watch`, … (DESIGN.md,
//! "Command line and terminal"). TeX's transcript and outputs are the same
//! as in compat mode; only the terminal differs.
//!
//! A subcommand resolves what to build (`config.rs`), makes the engine
//! command line a compat invocation would get (`pdflatex
//! -interaction=nonstopmode paper.tex`), and runs it through the same
//! session code as `-converge` and `-watch`, rendering what happens
//! (`render.rs`) instead of printing TeX's terminal stream.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

mod diffmode;
mod ssawatch;

use crate::config;
use crate::render::{End, Estimate, Rebuild, Renderer, Settings, Style};
use crate::term::{self, ColorChoice};

const USAGE: &str = "\
PhiTeX: a TeX engine that builds documents in one shot

usage: phitex <command> [options] [file.tex]

commands:
  build    build the document to its fixpoint (BibTeX and makeindex included)
  watch    build, then rebuild whenever an input changes, the pages live in
           the browser (on a terminal, keys: r rebuild, o open the viewer,
           w errors and warnings, d diff, b baseline, n/N next/previous
           change, D write the diff, q quit, ? help)
  check    compile once without writing the output files
  why      why the last build ran as it did, and every warning it gave
  trace    build, writing a Chrome/Perfetto timeline (--open: open Perfetto)
  clean    remove the files the last build wrote, its saved session and its
           saved build: the next build starts over
  clean --all
           remove every saved build, session and record, of every document
           (the formats stay); no file needed
  sync     sync FILE:LINE[:COL]: the watch's viewer shows that line

options:
  -o, --output-dir DIR     where the outputs go (TeX's -output-directory)
      --engine NAME        pdflatex (default for LaTeX), xelatex, pdftex, xetex,
                           latex or tex
      --shell-escape       let \\write18 run any command (default: restricted)
      --interactive        TeX's own terminal, stopping at errors (no fixpoint)
  -v, --verbose            show \\message and \\typeout lines, and what each pass
                           and rebuild ran (-vv: TeX's terminal)
  -q, --quiet              only problems and the result
      --color WHEN         auto, always or never (NO_COLOR and
                           CLICOLOR_FORCE are honoured)
      --message-format F   human or json
      --open               watch: open the PDF; trace: open Perfetto
      --no-view            watch: no live viewer in the browser (the
                           default on a terminal; --view: anywhere)
      --editor CMD         watch: where a double-click on a page opens its
                           source (code, emacsclient, nvim, … or a command
                           with {file} {line} {col}; else PHITEX_EDITOR,
                           VISUAL, EDITOR)
      --copy-pdf[=DIR]     build, watch: copy the PDF after each successful
                           build into DIR (default: where phitex was run)
      --diff REV           watch: show the diff against git revision REV
                           (HEAD, HEAD~3, a branch, a tag, a hash, or
                           `staged`), latexdiff's markup typeset beside the
                           document (`d` switches; machine runtime only)
      --no-machine         watch: rebuild through checkpoints instead of the
                           machine runtime (also PARTEX_MACHINE=0)
      --ssa                watch (experimental): rebuild on the dynamic-SSA
                           runtime, the Overleaf extension's engine: an edit
                           runs one pass and shows its pages, the passes
                           after it settle while idle. It keeps no store:
                           each watch --ssa starts with a cold build
      --no-synctex         build, watch: no <job>.synctex.gz (written by
                           default, as pdflatex -synctex=1 writes it, for
                           an editor's forward and inverse search;
                           --synctex: on again)
  -V, --version
  -h, --help

Without a file, `phitex.toml` (here or above) names it:
    main = \"paper.tex\"   engine = \"pdflatex\"   output-dir = \"out\"
    copy-pdf = true   (or a directory, relative to phitex.toml's)
    machine = false   (watch: the checkpoint rebuilds, as --no-machine)
    ssa = true        (watch: the dynamic-SSA runtime, as --ssa)
    synctex = false   (no <job>.synctex.gz, as --no-synctex)
A `% !TEX program = …` or `% !TEX root = …` comment in the file is honoured.

For TeX's own command line use `phitex --compat=pdftex …` (or `tex`), or
run phitex as `pdftex`, `pdflatex` or `tex`.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Build,
    Watch,
    Check,
    Why,
    Trace,
    Clean,
    Sync,
}

/// The modern command line's options.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
struct Options {
    command: Command,
    file: Option<String>,
    engine: Option<String>,
    output_dir: Option<String>,
    shell_escape: Option<bool>,
    interactive: bool,
    verbose: u8,
    quiet: bool,
    color: ColorChoice,
    json: bool,
    open: bool,
    /// `phitex trace --output FILE`.
    timeline: Option<String>,
    /// `--copy-pdf[=DIR]` or `--no-copy-pdf` (a relative `DIR` is relative
    /// to the invocation directory).
    copy_pdf: Option<config::CopyPdf>,
    /// `--no-machine`.
    no_machine: bool,
    /// `--ssa`.
    ssa: bool,
    /// `clean --all`.
    all: bool,
    /// `--view` or `--no-view` (none: the viewer on a terminal).
    view: Option<bool>,
    /// `--editor`.
    editor: Option<String>,
    /// `--synctex` or `--no-synctex` (none: `phitex.toml`'s, else on).
    synctex: Option<bool>,
    /// `watch --diff REV`.
    diff: Option<String>,
}

#[allow(clippy::too_many_lines, reason = "one option a line")]
fn parse(args: &[String]) -> Result<Options, String> {
    let command = match args.first().map(String::as_str) {
        Some("build" | "b") => Command::Build,
        Some("watch" | "w") => Command::Watch,
        Some("check" | "c") => Command::Check,
        Some("why") => Command::Why,
        Some("trace") => Command::Trace,
        Some("clean") => Command::Clean,
        Some("sync") => Command::Sync,
        Some("-h" | "--help" | "help") | None => return Err(String::new()),
        Some(a) if a.starts_with('-') || a.starts_with('&') || a.starts_with('\\') => {
            return Err(format!(
                "`{a}` is a TeX command-line argument; use `phitex --compat=tex {a} …` \
                 (or `--compat=pdftex`) for TeX's command line"
            ));
        }
        Some(c) => return Err(format!("unknown command `{c}`")),
    };
    let mut o = Options {
        command,
        file: None,
        engine: None,
        output_dir: None,
        shell_escape: None,
        interactive: false,
        verbose: 0,
        quiet: false,
        color: ColorChoice::Auto,
        json: false,
        open: false,
        timeline: None,
        copy_pdf: None,
        no_machine: false,
        ssa: false,
        all: false,
        view: None,
        editor: None,
        synctex: None,
        diff: None,
    };
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        let mut value = || -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| format!("`{name}` needs a value"))
        };
        match name {
            "-o" | "--output-dir" | "--output-directory" | "--out-dir" => {
                o.output_dir = Some(value()?);
            }
            "--engine" | "--program" => o.engine = Some(value()?),
            "--shell-escape" => o.shell_escape = Some(true),
            "--no-shell-escape" => o.shell_escape = Some(false),
            "--interactive" => o.interactive = true,
            "-v" | "--verbose" => o.verbose += 1,
            "-vv" => o.verbose += 2,
            "-q" | "--quiet" => o.quiet = true,
            "--color" | "--colour" => {
                o.color = match value()?.as_str() {
                    "auto" => ColorChoice::Auto,
                    "always" => ColorChoice::Always,
                    "never" => ColorChoice::Never,
                    v => return Err(format!("--color: `{v}` is not auto, always or never")),
                };
            }
            "--message-format" => {
                o.json = match value()?.as_str() {
                    "human" | "short" => false,
                    "json" => true,
                    v => return Err(format!("--message-format: `{v}` is not human or json")),
                };
            }
            "--open" => o.open = true,
            // (the value only inline: `--copy-pdf paper.tex` names the file)
            "--copy-pdf" => {
                o.copy_pdf = Some(match &inline {
                    Some(d) if d.is_empty() => return Err("`--copy-pdf=` needs a directory".into()),
                    Some(d) => config::CopyPdf::Dir(d.clone()),
                    None => config::CopyPdf::Invocation,
                });
            }
            "--no-copy-pdf" => o.copy_pdf = Some(config::CopyPdf::Off),
            "--no-machine" => o.no_machine = true,
            "--ssa" => o.ssa = true,
            "--no-ssa" => o.ssa = false,
            "--view" => o.view = Some(true),
            "--no-view" => o.view = Some(false),
            "--editor" => o.editor = Some(value()?),
            "--synctex" => o.synctex = Some(true),
            "--no-synctex" => o.synctex = Some(false),
            "--machine" => o.no_machine = false,
            "--output" if o.command == Command::Trace => o.timeline = Some(value()?),
            "--diff" if o.command == Command::Watch => o.diff = Some(value()?),
            "--all" if o.command == Command::Clean => o.all = true,
            "-h" | "--help" => return Err(String::new()),
            f if f.starts_with('-') && f.len() > 1 => {
                return Err(format!("unknown option `{f}`"));
            }
            f => {
                if o.file.replace(f.to_owned()).is_some() {
                    return Err(String::from("more than one file given"));
                }
            }
        }
    }
    Ok(o)
}

/// What to build.
#[derive(Debug)]
struct Target {
    /// The main file, as TeX is given it.
    file: String,
    /// The engine's program name (`pdflatex`, …).
    engine: String,
    output_dir: Option<String>,
    shell_escape: Option<bool>,
    viewer: Option<String>,
    /// The directory to copy the final PDF into after each successful
    /// build (absolute: the build runs in `phitex.toml`'s directory).
    copy_pdf: Option<PathBuf>,
    /// `phitex watch` in machine mode: the default, unless `--no-machine`,
    /// `machine = false` or `PARTEX_MACHINE=0` says otherwise.
    machine: bool,
    /// `phitex watch` on the dynamic-SSA runtime (`--ssa`, `ssa = true`).
    ssa: bool,
    /// Whether the build writes `<job>.synctex.gz` (`-synctex=1`; unless
    /// `--no-synctex` or `synctex = false`).
    synctex: bool,
}

impl Target {
    /// The job name: the main file's name without its directory and `.tex`.
    fn job(&self) -> String {
        let base = self.file.rsplit('/').next().unwrap_or(&self.file);
        base.strip_suffix(".tex").unwrap_or(base).to_owned()
    }

    /// Where an output of the job named `name` is.
    fn output(&self, name: &str) -> PathBuf {
        match &self.output_dir {
            Some(d) => Path::new(d).join(name),
            None => PathBuf::from(name),
        }
    }

    /// The engine command line a compat invocation would get: with
    /// `-synctex=1` unless `SyncTeX` is off.
    fn engine_args(&self, interaction: &str) -> Vec<String> {
        self.engine_args_with(interaction, self.synctex)
    }

    /// The engine command line, with `-synctex=1` if `synctex`.
    fn engine_args_with(&self, interaction: &str, synctex: bool) -> Vec<String> {
        let mut a = vec![self.engine.clone(), format!("-interaction={interaction}")];
        if synctex {
            a.push("-synctex=1".into());
        }
        if let Some(d) = &self.output_dir {
            a.push(format!("-output-directory={d}"));
        }
        match self.shell_escape {
            Some(true) => a.push("-shell-escape".into()),
            Some(false) => a.push("-no-shell-escape".into()),
            None => {}
        }
        a.push(self.file.clone());
        a
    }
}

/// The engines `phitex` has, by the names a magic comment or `phitex.toml`
/// may use.
fn engine_name(name: &str) -> Result<String, String> {
    let n = name.trim().to_ascii_lowercase();
    match n.as_str() {
        "pdflatex" | "latex" | "pdftex" | "tex" | "etex" | "xelatex" | "xetex" => Ok(n),
        "lualatex" | "luatex" | "lualatex-dev" | "xelatex-dev" => {
            Err(format!("the {n} engine is not supported yet"))
        }
        _ => Err(format!("unknown engine `{name}`")),
    }
}

/// Resolve what to build: the command line, then `phitex.toml` (running in
/// its directory), then the file's magic comments.
fn resolve(o: &Options) -> Result<Target, String> {
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
    if let Some(s) = &cfg.store {
        crate::machinehost::persisted::configure(root.join(s));
    }
    // (the flag's directory is the invocation's, the key's partex.toml's)
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
        std::fs::read_to_string(f)
            .or_else(|_| std::fs::read_to_string(format!("{f}.tex")))
            .map_err(|e| format!("can't read {f}: {e}"))
    };
    let mut text = read(&file)?;
    let (mut program, root) = config::magic_comments(&text);
    if let Some(root) = root {
        let dir = Path::new(&file).parent().unwrap_or(Path::new(""));
        file = dir.join(root).to_string_lossy().into_owned();
        text = read(&file)?;
        program = config::magic_comments(&text).0.or(program);
    }
    let engine = match o.engine.clone().or(cfg.engine).or(program) {
        Some(e) => engine_name(&e)?,
        None => config::default_engine(&text).to_owned(),
    };
    // (without a phitex.toml, the main file's directory is the project's,
    // as for latexmk -cd or Overleaf: TeX finds `\input{preamble}` beside
    // `book/book.tex`; the flags' paths stay the invocation's)
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
    let machine = !o.no_machine
        && cfg.machine != Some(false)
        && !std::env::var("PARTEX_MACHINE").is_ok_and(|v| v == "0");
    let ssa = o.ssa || cfg.ssa == Some(true);
    Ok(Target {
        file,
        engine,
        output_dir,
        shell_escape: o.shell_escape.or(cfg.shell_escape),
        viewer: cfg.viewer,
        copy_pdf,
        machine,
        ssa,
        // (the runtimes that keep it: the machine's and the SSA watch's;
        // not a checkpoint session's, `session`)
        synctex: o.synctex.or(cfg.synctex).unwrap_or(true) && (machine || ssa),
    })
}

/// Where `copy-pdf` `c` copies to: `cwd` is the invocation directory,
/// `base` what a relative directory is relative to.
fn copy_dir(c: &config::CopyPdf, cwd: &Path, base: &Path) -> Option<PathBuf> {
    match c {
        config::CopyPdf::Off => None,
        config::CopyPdf::Invocation => Some(cwd.to_path_buf()),
        config::CopyPdf::Dir(d) => Some(base.join(d)),
    }
}

fn settings(o: &Options) -> Settings {
    let caps = term::Caps::detect(o.color);
    Settings {
        style: Style {
            color: caps.color,
            unicode: caps.unicode,
            links: caps.links && caps.color,
        },
        progress: caps.tty && !o.json,
        verbose: o.verbose,
        quiet: o.quiet,
    }
}

fn fail(msg: &str, style: Style) -> ! {
    eprintln!("{}: {msg}", style.red("error"));
    term::exit(2);
}

/// Run the modern command line with `args`.
pub fn main(args: &[String]) -> ! {
    crate::set_modern();
    if matches!(args.first().map(String::as_str), Some("-V" | "--version")) {
        println!("PhiTeX {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            std::process::exit(if args.is_empty() { 2 } else { 0 });
        }
        Err(e) => {
            eprintln!("phitex: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let st = settings(&o);
    if o.all {
        clean_all(st);
    }
    if o.command == Command::Sync {
        let Some(place) = o.file.as_deref() else {
            fail("sync: give FILE:LINE[:COL]", st.style)
        };
        std::process::exit(crate::view::sync_command(place));
    }
    let target = resolve(&o).unwrap_or_else(|e| fail(&e, st.style));
    match o.command {
        Command::Build | Command::Check | Command::Trace if o.interactive => interactive(&target),
        Command::Build => std::process::exit(build(&target, st, false)),
        Command::Check => std::process::exit(build(&target, st, true)),
        Command::Trace => trace(&o, &target, st),
        Command::Watch => watch(&o, &target, st),
        Command::Why => why(&target, st),
        Command::Clean => clean(&target, st),
        Command::Sync => unreachable!("answered above"),
    }
}

/// `--interactive`: TeX's own terminal and prompt, one pass.
fn interactive(t: &Target) -> ! {
    let formats = ensure_format(&t.engine, None);
    crate::set_args(t.engine_args("errorstopmode"));
    let mut job = crate::setup();
    job.host.formats = formats;
    crate::make_output_dir(&job.host);
    let h = crate::run_memo(job.host, job.params, job.command_line.as_bytes());
    std::process::exit(i32::from(h > 1));
}

/// The format `engine` loads, made in the cache's formats directory if it
/// is not there yet (as fmtutil makes TeX Live's): that directory, or
/// `None` without a cache (kpathsea's formats are used then).
fn ensure_format(engine: &str, r: Option<&Renderer>) -> Option<PathBuf> {
    let dir = crate::cache::formats_dir()?;
    if dir.join(format!("{engine}.fmt")).is_file() {
        return Some(dir);
    }
    // (fmtutil.cnf's lines for these formats)
    let (flavor, ini) = match engine {
        "tex" => ("tex", "tex.ini"),
        "etex" => ("pdftex", "*etex.ini"),
        "pdftex" => ("pdftex", "*pdfetex.ini"),
        "latex" => ("pdftex", "*latex.ini"),
        "xetex" => ("xetex", "xetex.ini"),
        "xelatex" => ("xetex", "xelatex.ini"),
        _ => ("pdftex", "*pdflatex.ini"),
    };
    if let Some(r) = r {
        r.status(
            "Building",
            &format!("{engine} format (first run of this PhiTeX version)"),
        );
        r.task("Building", &format!("{engine} format"), true);
    }
    let mut args = vec![
        flavor.to_owned(),
        "-ini".into(),
        "-interaction=batchmode".into(),
        format!("-jobname={engine}"),
        format!("-output-directory={}", dir.display()),
    ];
    match flavor {
        "pdftex" => args.push("-translate-file=cp227.tcx".into()),
        "xetex" => args.push("-etex".into()),
        _ => {}
    }
    args.push(ini.into());
    crate::set_args(args);
    let job = crate::setup();
    let mut s = crate::session::Session::new(
        job.params,
        job.command_line.into_bytes(),
        job.host,
        u64::MAX / 2,
    );
    let rep = s.build();
    let _ = s.write_outputs();
    if let Some(r) = r {
        r.idle();
    }
    if !dir.join(format!("{engine}.fmt")).is_file() || rep.history > 2 {
        eprintln!(
            "phitex: making the {engine} format failed; see {}",
            dir.join(format!("{engine}.log")).display()
        );
        return None;
    }
    Some(dir)
}

/// A session for target `t`, rendering to `r`.
fn session(t: &Target, r: &Renderer) -> crate::session::Session {
    let formats = ensure_format(&t.engine, Some(r));
    // (no `SyncTeX`: a checkpoint session cannot keep it, DESIGN 4.5)
    crate::set_args(t.engine_args_with("nonstopmode", false));
    let mut job = crate::setup();
    job.host.formats = formats;
    job.host.notes = true;
    crate::make_output_dir(&job.host);
    crate::session::Session::new(
        job.params,
        job.command_line.into_bytes(),
        job.host,
        job.checkpoint_every,
    )
}

/// The main output file among `outputs` (the PDF, else the DVI file), for
/// display: web2c joins `-output-directory` and the name with a `/` even
/// if the directory ends in one, and the log keeps that `//`.
fn main_output(outputs: &[(Vec<u8>, usize)]) -> Option<String> {
    let name = |ext: &[u8]| {
        outputs
            .iter()
            .find(|(n, len)| n.ends_with(ext) && *len > 0)
            .map(|(n, _)| String::from_utf8_lossy(n).into_owned())
    };
    let mut path = name(b".pdf").or_else(|| name(b".dvi"))?;
    while path.contains("//") {
        path = path.replace("//", "/");
    }
    Some(path)
}

/// `phitex build` in machine mode with the store on (DESIGN.md §7.9):
/// the build saved for the job, rebuilt after what changed since (else
/// built afresh), then saved again, after the result: the exit status.
fn machine_build(t: &Target, st: Settings) -> i32 {
    let ren = Renderer::new(st);
    ren.status("Compiling", &format!("{} ({})", t.file, t.engine));
    let formats = ensure_format(&t.engine, Some(&ren));
    crate::set_args(t.engine_args("nonstopmode"));
    let mut job = crate::setup();
    job.host.formats = formats;
    job.host.notes = true;
    crate::make_output_dir(&job.host);
    ren.set_estimate(load_estimate());
    ren.start();
    let mut between = crate::Between::default();
    let (mut w, out) = crate::machinehost::Watch::open(
        job.host,
        job.params,
        job.command_line.as_bytes(),
        &mut between,
        &mut |p| ren.progress(&p),
        false,
    );
    let status = machine_finish(t, &ren, &out, None, false);
    save_estimate(&ren);
    ren.progress(&crate::events::Progress::Phase(
        crate::events::Phase::Saving,
    ));
    w.finish_saving();
    ren.idle();
    store_hint(&ren);
    // (the build is not torn down: the process ends)
    std::mem::forget(w);
    status
}

/// Whether `phitex build` goes through the machine and the store.
fn machine_builds(t: &Target) -> bool {
    t.machine && crate::store::dir(None).is_some()
}

/// `phitex build` (or `check`: one pass, nothing written): the exit status.
fn build(t: &Target, st: Settings, check: bool) -> i32 {
    if !check && machine_builds(t) {
        return machine_build(t, st);
    }
    let r = Renderer::new(st);
    let verb = if check { "Checking" } else { "Compiling" };
    r.status(verb, &format!("{} ({})", t.file, t.engine));
    let mut s = session(t, &r);
    r.set_estimate(load_estimate());
    r.start();
    let (reports, term, h) = if check {
        r.progress(&crate::events::Progress::PassStart(1));
        r.progress(&crate::events::Progress::Phase(crate::events::Phase::Cold));
        let rep = s.build();
        r.progress(&crate::events::Progress::Pass(1, Some(&rep)));
        (Vec::new(), s.terminal(), rep.history)
    } else {
        crate::converge_saved(&mut s, &mut crate::Between::default(), &mut |p| {
            r.progress(&p);
        })
    };
    let status = finish(t, &r, &s, &reports, &term, h, check, None);
    save_estimate(&r);
    status
}

/// Where the totals of the last build of this job from its start are
/// kept (by directory and engine command line: not by day, as a saved
/// session is).
fn estimate_key() -> u128 {
    let dir = std::env::current_dir().unwrap_or_default();
    partex_core::persist_hash(&(
        "estimate/1",
        dir.as_os_str().as_encoded_bytes(),
        crate::args(),
    ))
}

/// The totals of the last build of this job from its start.
fn load_estimate() -> Option<Estimate> {
    let text = crate::cache::get(estimate_key())?;
    Estimate::parse(std::str::from_utf8(&text).ok()?)
}

/// Keep this build's totals for the next, if it ran from the start.
fn save_estimate(r: &Renderer) {
    if let Some(e) = r.measured() {
        crate::cache::put(estimate_key(), e.to_text().as_bytes());
    }
}

/// The size of the file named `name` among `outputs`.
fn output_bytes(outputs: &[(Vec<u8>, usize)], name: Option<&str>) -> Option<usize> {
    let name = name?;
    outputs.iter().find_map(|(n, len)| {
        let n = String::from_utf8_lossy(n);
        (n.replace("//", "/") == name).then_some(*len)
    })
}

/// Render the end of a build and record it for `phitex why` and `partex
/// clean`: the exit status.
#[allow(clippy::too_many_arguments)]
fn finish(
    t: &Target,
    r: &Renderer,
    s: &crate::session::Session,
    reports: &[String],
    term: &[u8],
    h: i32,
    check: bool,
    rebuild: Option<Rebuild>,
) -> i32 {
    let summary = crate::warnings::summarize(&s.diagnostics());
    let outputs = s.outputs();
    let output = if check { None } else { main_output(&outputs) };
    r.finish(&End {
        file: &t.file,
        summary: &summary,
        term,
        bytes: output_bytes(&outputs, output.as_deref()),
        output,
        failed: h > 1,
        checked: check,
        rebuild,
    });
    if !check {
        record(t, reports, &summary, &outputs);
        if h <= 1 {
            copy_final_pdf(t, r, &outputs);
        }
    }
    i32::from(h > 1)
}

/// `--copy-pdf`: copy the PDF among `outputs` where target `t` says.
fn copy_final_pdf(t: &Target, r: &Renderer, outputs: &[(Vec<u8>, usize)]) {
    let Some(dir) = &t.copy_pdf else { return };
    let Some(pdf) =
        main_output(outputs).filter(|p| Path::new(p).extension().is_some_and(|ext| ext == "pdf"))
    else {
        return;
    };
    match copy_pdf(Path::new(&pdf), dir) {
        Ok(Some(to)) => r.status("Copied", &format!("{pdf} to {}", to.display())),
        Ok(None) => {}
        Err(err) => r.status("Not copied", &format!("{pdf} to {}: {err}", dir.display())),
    }
}

/// Copy `pdf` into `dir` atomically (a temporary file there, renamed over
/// the old copy, so a viewer never sees half a PDF): where it went, or
/// `None` if that is `pdf` itself.
fn copy_pdf(pdf: &Path, dir: &Path) -> std::io::Result<Option<PathBuf>> {
    let name = pdf
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?;
    let to = dir.join(name);
    if let (Ok(a), Ok(b)) = (pdf.canonicalize(), to.canonicalize())
        && a == b
    {
        return Ok(None);
    }
    std::fs::create_dir_all(dir)?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".phitex-{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);
    let done = std::fs::copy(pdf, &tmp).and_then(|_| std::fs::rename(&tmp, &to));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done.map(|()| Some(to))
}

/// Where `phitex why` finds what the last build of the same job did.
fn record_key() -> Option<u128> {
    crate::saved_session_key().map(|k| partex_core::persist_hash(&("why/1", k)))
}

/// Keep what a build did for `phitex why` and `phitex clean`.
fn record(
    t: &Target,
    reports: &[String],
    summary: &crate::warnings::Summary,
    outputs: &[(Vec<u8>, usize)],
) {
    use std::fmt::Write as _;
    let Some(key) = record_key() else { return };
    let mut text = String::new();
    let _ = writeln!(text, "file {}", t.file);
    for r in reports {
        for line in r.lines() {
            let _ = writeln!(text, "report {line}");
        }
    }
    for g in &summary.groups {
        let _ = writeln!(text, "warning {}", g.title());
        for i in &g.items {
            let locs: Vec<String> = i.locations.iter().map(ToString::to_string).collect();
            let _ = writeln!(text, "at {}", locs.join(", "));
            let _ = writeln!(text, "subject {}", i.subject.replace('\n', " "));
            if !i.excerpt.is_empty() {
                let _ = writeln!(text, "excerpt {}", i.excerpt.replace('\n', " "));
            }
        }
    }
    for (name, _) in outputs {
        let _ = writeln!(text, "output {}", String::from_utf8_lossy(name));
    }
    crate::cache::put(key, text.as_bytes());
}

/// The record of the last build of this job, set up as a build would be.
fn last_record(t: &Target) -> Option<String> {
    crate::set_args(t.engine_args("nonstopmode"));
    let text = crate::cache::get(record_key()?)?;
    String::from_utf8(text).ok()
}

/// `phitex why`.
fn why(t: &Target, st: Settings) -> ! {
    let s = st.style;
    let Some(rec) = last_record(t) else {
        fail(
            &format!("no build of {} recorded yet (run `phitex build`)", t.file),
            s,
        );
    };
    let mut warnings = 0;
    for line in rec.lines() {
        let (tag, rest) = line.split_once(' ').unwrap_or((line, ""));
        match tag {
            "file" => println!("{} {rest}", s.green(&format!("{:>12}", "Last build"))),
            "report" => {
                let rest = rest
                    .strip_prefix("phitex:")
                    .or_else(|| rest.strip_prefix("partex:"))
                    .unwrap_or(rest);
                match rest.strip_prefix("   ") {
                    Some(detail) => println!("{:>12}   {}", "", s.dim(detail)),
                    None => println!("{:>12} {}", "", rest.trim_start()),
                }
            }
            "warning" => {
                warnings += 1;
                println!("{}: {}", s.yellow("warning"), s.bold(rest));
            }
            "at" => println!("{} {rest}", s.blue("  -->")),
            "subject" => println!("{} {rest}", s.blue("   |")),
            "excerpt" => println!("{} {}", s.blue("   |"), s.dim(rest)),
            _ => {}
        }
    }
    if warnings == 0 {
        println!("{:>12} no warnings", "");
    }
    std::process::exit(0);
}

/// `phitex clean`: remove what the last build wrote (the job's usual
/// outputs if none is recorded), its saved session, and its saved build
/// and no-op record in the store: the next build starts over. Then how
/// much the store keeps for other documents.
fn clean(t: &Target, st: Settings) -> ! {
    let rec = last_record(t);
    let mut files: Vec<PathBuf> = rec
        .iter()
        .flat_map(|r| r.lines())
        .filter_map(|l| l.strip_prefix("output "))
        .map(PathBuf::from)
        .collect();
    if files.is_empty() {
        let job = t.job();
        for ext in [
            "aux",
            "log",
            "toc",
            "lof",
            "lot",
            "out",
            "bbl",
            "blg",
            "idx",
            "ind",
            "ilg",
            "pdf",
            "dvi",
            "nav",
            "snm",
            "vrb",
            "synctex.gz",
            "synctex",
        ] {
            files.push(t.output(&format!("{job}.{ext}")));
        }
    }
    let removed = files
        .iter()
        .filter(|f| std::fs::remove_file(f).is_ok())
        .count();
    let mut what = vec![crate::render::plural(removed, "file", "files")];
    let mut session = false;
    for key in [crate::saved_session_key(), record_key()]
        .into_iter()
        .flatten()
    {
        if let Some(p) = crate::cache::path(key) {
            session |= std::fs::remove_file(p).is_ok();
        }
    }
    if session {
        what.push("the saved session".into());
    }
    // (the store keys a job as a build does: by its engine command line,
    // which `last_record` set)
    let job = crate::setup();
    let store = crate::store::dir(crate::machinehost::persisted::configured()).map(|dir| {
        let before = bytes_under(&dir);
        let forgot =
            crate::machinehost::persisted::forget(&job.params, job.command_line.as_bytes());
        let after = bytes_under(&dir);
        (
            dir,
            forgot.is_some_and(|(_, f)| f),
            before.saturating_sub(after),
            after,
        )
    });
    if let Some((_, true, freed, _)) = &store {
        let freed = if *freed > 0 {
            format!(
                " ({})",
                crate::render::size(usize::try_from(*freed).unwrap_or(usize::MAX))
            )
        } else {
            String::new()
        };
        what.push(format!("the saved build{freed}"));
    }
    let r = Renderer::new(st);
    r.status("Removed", &and_list(&what));
    if let Some((dir, _, _, left)) = store
        && left > 0
    {
        let left = crate::render::size(usize::try_from(left).unwrap_or(usize::MAX));
        r.status(
            "Store",
            &format!(
                "{} holds {left} for other documents{}`phitex clean --all` removes it all",
                tilde(&dir),
                st.style.sep()
            ),
        );
    }
    std::process::exit(0);
}

/// `phitex clean --all`: every saved build (the whole store), saved
/// session and record, of every document; the formats stay. What the
/// builds named partex kept goes too.
fn clean_all(st: Settings) -> ! {
    let mut freed = 0;
    let mut places = Vec::new();
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(Some((root, cfg))) = config::find(&cwd)
        && let Some(s) = &cfg.store
    {
        crate::machinehost::persisted::configure(root.join(s));
    }
    if let Some(dir) = crate::store::dir(crate::machinehost::persisted::configured())
        && dir.exists()
    {
        freed += bytes_under(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        places.push(tilde(&dir));
    }
    // (the cache's values: sessions, records, estimates; not its formats)
    if let Some(dir) = crate::cache::dir() {
        let mut any = false;
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if let Ok(m) = e.metadata()
                && m.is_file()
                && std::fs::remove_file(e.path()).is_ok()
            {
                freed += m.len();
                any = true;
            }
        }
        if any {
            places.push(tilde(&dir));
        }
    }
    if let Some(dir) = crate::cache::legacy_dir() {
        freed += bytes_under(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        places.push(tilde(&dir));
    }
    let r = Renderer::new(st);
    if places.is_empty() {
        r.status("Removed", "nothing: no saved builds, sessions or records");
    } else {
        let freed = crate::render::size(usize::try_from(freed).unwrap_or(usize::MAX));
        r.status(
            "Removed",
            &format!(
                "every saved build, session and record ({freed}) in {}",
                and_list(&places)
            ),
        );
    }
    std::process::exit(0);
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// After a build is saved (a `build`'s end, a watch's quit): what the
/// store and the cache keep for every document, how they clean
/// themselves, and the commands that clean them now.
fn store_hint(ren: &Renderer) {
    use std::fmt::Write as _;
    let size = |b: u64| crate::render::size(usize::try_from(b).unwrap_or(usize::MAX));
    let store = crate::store::dir(crate::machinehost::persisted::configured());
    let kept = store.as_deref().map_or(0, bytes_under)
        + crate::cache::dir().as_deref().map_or(0, bytes_under);
    if kept == 0 {
        return;
    }
    let age = crate::store::max_age().map_or(String::new(), |a| {
        format!(", unused for {} days removed", a.as_secs() / (24 * 3600))
    });
    let mut text = format!(
        "{} of saved builds and caches ({} at most{age}) · `phitex clean` this document · `phitex clean --all` every one",
        size(kept),
        size(crate::store::max() + crate::cache::max())
    );
    if let Some(old) = crate::cache::legacy_dir() {
        let _ = write!(
            text,
            " (and {} unused, of the builds named partex, in {})",
            size(bytes_under(&old)),
            tilde(&old)
        );
    }
    ren.status("Kept", &text);
}

/// The bytes of the files under `path`: one walk, their lengths only.
fn bytes_under(path: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !m.is_dir() {
        return m.len();
    }
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| bytes_under(&e.path()))
        .sum()
}

/// `path` with `~` for the home directory.
fn tilde(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return Path::new("~").join(rest).display().to_string();
    }
    path.display().to_string()
}

/// `phitex trace`: build, writing the timeline.
fn trace(o: &Options, t: &Target, st: Settings) -> ! {
    let path = PathBuf::from(
        o.timeline
            .clone()
            .unwrap_or_else(|| format!("{}.trace.json", t.job())),
    );
    crate::timeline::record_to(path.clone());
    let status = build(t, st, false);
    crate::timeline::finish();
    let r = Renderer::new(st);
    r.status(
        "Timeline",
        &format!(
            "{} (Chrome trace JSON: open it in https://ui.perfetto.dev or chrome://tracing)",
            path.display()
        ),
    );
    if o.open {
        open_url("https://ui.perfetto.dev/");
    }
    std::process::exit(status);
}

/// Show `what` with the desktop's default program (best effort).
fn open_url(what: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(what)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// What a watch reads from its user: keys on a terminal, else lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    /// `q`, Ctrl-D, Ctrl-C (or a line `q`): stop, after saving the build.
    Quit,
    /// `r`: look at every input now, and rebuild if one changed.
    Rebuild,
    /// `o`: open the PDF.
    Open,
    /// `w`: the last build's errors and warnings again.
    Problems,
    /// `c`, Ctrl-L: clear the screen.
    Clear,
    /// `d`: the diff against the baseline shown, or the document again.
    Diff,
    /// `b`: choose the baseline, from the repository's commits.
    Baseline,
    /// `n` and `N` (`p`): the next and the previous change.
    Next,
    Prev,
    /// `D`: the diff written beside the outputs (`<job>-diff.tex`, `.pdf`).
    WriteDiff,
    /// The arrows (and `j`, `k`), Enter and Esc: a list's.
    Up,
    Down,
    Enter,
    Esc,
    /// The live viewer asked for something (`View::take_asks`).
    Viewer,
}

impl Input {
    /// Whether each press counts (a list's steps), not only the first of
    /// a build's time.
    fn repeats(self) -> bool {
        matches!(self, Input::Up | Input::Down | Input::Next | Input::Prev)
    }
}

/// What `?` shows (the status line says only `? help`).
const KEYS: [&str; 3] = [
    "r rebuild now · o open the viewer (or the PDF) · w the errors and warnings · c clear the screen",
    "d diff against the baseline (or back) · b choose the baseline · n/N next/previous change · D write <job>-diff.tex and .pdf",
    "q quit, after saving the build (Ctrl-C too; twice: at once) · Ctrl-Z suspend",
];

/// A watch is building (Ctrl-C then stops it once the build is over).
static BUSY: AtomicBool = AtomicBool::new(false);
/// Ctrl-C was pressed (a second one stops the watch at once).
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// An escape sequence at the start of `rest` (after its ESC): what it
/// means, and its length. ESC alone is Esc; a sequence not known (a
/// function key) means nothing.
fn escape(rest: &[u8]) -> (Option<Input>, usize) {
    match rest {
        [b'[' | b'O', b'A', ..] => (Some(Input::Up), 2),
        [b'[' | b'O', b'B', ..] => (Some(Input::Down), 2),
        [b'[', tail @ ..] => {
            // (CSI: parameters, then a final byte)
            let n = tail
                .iter()
                .position(|c| (0x40..=0x7e).contains(c))
                .map_or(tail.len(), |k| k + 1);
            (None, 1 + n)
        }
        [b'O', _, ..] => (None, 2),
        // (ESC alone; or Alt and a key, Esc then a key: the Esc)
        _ => (Some(Input::Esc), 0),
    }
}

/// What a line means to a watch reading lines (not keys): `q`, and the
/// diff's `d`, `n`, `N`, `D` (scripts and tests).
fn line_input(line: &str) -> Option<Input> {
    Some(match line.trim() {
        "q" => Input::Quit,
        "r" => Input::Rebuild,
        "d" => Input::Diff,
        "n" => Input::Next,
        "N" | "p" => Input::Prev,
        "D" => Input::WriteDiff,
        _ => return None,
    })
}

/// Read the user's keys (`keys`: the terminal's modes are set for it) or
/// lines into `tx`, on a thread of its own, from the watch's start: a key
/// pressed while the first build runs is not echoed into the live line,
/// and `?` is answered at once.
fn read_input(keys: bool, printer: std::sync::Arc<crate::live::Live>, tx: mpsc::Sender<Input>) {
    use std::io::Read;
    use std::sync::atomic::Ordering::Relaxed;
    std::thread::spawn(move || {
        if !keys {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { break };
                if let Some(i) = line_input(&line)
                    && tx.send(i).is_err()
                {
                    break;
                }
            }
            return;
        }
        let mut buf = [0u8; 64];
        let mut stdin = std::io::stdin().lock();
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Input::Quit);
                    return;
                }
                Ok(n) => n,
            };
            let mut at = 0;
            while at < n {
                let b = buf[at];
                at += 1;
                let input = match b {
                    // (an escape sequence: an arrow, a function key)
                    0x1b => {
                        let (i, len) = escape(&buf[at..n]);
                        at += len;
                        match i {
                            Some(i) => i,
                            None => continue,
                        }
                    }
                    b'q' | b'Q' | 0x04 => Input::Quit,
                    0x03 => {
                        if INTERRUPTED.swap(true, Relaxed) {
                            printer.close();
                            term::exit(130);
                        }
                        if BUSY.load(Relaxed) {
                            printer.warn("Stopping", "once this build is over (Ctrl-C again: now)");
                        }
                        Input::Quit
                    }
                    0x1c => {
                        printer.close();
                        term::exit(131);
                    }
                    0x1a => {
                        printer.suspend();
                        continue;
                    }
                    b'r' | b'R' => Input::Rebuild,
                    b'o' | b'O' => Input::Open,
                    b'w' | b'W' | b'e' | b'E' => Input::Problems,
                    b'c' | b'C' | 0x0c => Input::Clear,
                    b'd' => Input::Diff,
                    b'D' => Input::WriteDiff,
                    b'b' | b'B' => Input::Baseline,
                    b'n' => Input::Next,
                    b'N' | b'p' | b'P' => Input::Prev,
                    b'k' => Input::Up,
                    b'j' => Input::Down,
                    b'\r' | b'\n' => Input::Enter,
                    b'?' | b'h' | b'H' => {
                        printer.help(&KEYS);
                        continue;
                    }
                    _ => continue,
                };
                if tx.send(input).is_err() {
                    return;
                }
            }
        }
    });
}

/// The watch's polling period (`PARTEX_WATCH_POLL_MS`, `default` ms).
fn poll_period(default: u64) -> Duration {
    Duration::from_millis(
        std::env::var("PARTEX_WATCH_POLL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default),
    )
}

/// The inputs waiting after up to `poll` (none: time to look at the
/// files), each once: keys pressed while a build ran are answered once,
/// not once a press. A watch reading no more lines sleeps instead.
fn next_inputs(rx: &mpsc::Receiver<Input>, poll: Duration) -> Vec<Input> {
    let first = match rx.recv_timeout(poll) {
        Ok(i) => i,
        Err(mpsc::RecvTimeoutError::Timeout) => return Vec::new(),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            std::thread::sleep(poll);
            return Vec::new();
        }
    };
    let mut all = vec![first];
    while let Ok(i) = rx.try_recv() {
        if i.repeats() || !all.contains(&i) {
            all.push(i);
        }
    }
    all
}

/// What a watch does with `input` that is not a rebuild or a stop
/// (`outputs`: the last build's files).
fn answer(
    input: Input,
    ren: &Renderer,
    target: &Target,
    outputs: &[(Vec<u8>, usize)],
    viewer: &mut Viewer,
) {
    match input {
        Input::Open => viewer.open(ren, target, outputs),
        Input::Problems => ren.show_problems(),
        Input::Clear => ren.live().clear_screen(),
        // (the diff's, answered by the machine-mode watch)
        Input::Diff
        | Input::Baseline
        | Input::Next
        | Input::Prev
        | Input::WriteDiff
        | Input::Up
        | Input::Down
        | Input::Enter
        | Input::Esc
        | Input::Viewer
        | Input::Quit
        | Input::Rebuild => {}
    }
}

/// How a watch shows its PDF (`o`, `--open`): the live viewer in the
/// browser (DESIGN 4.8) if it is on, else the viewer `phitex.toml` names,
/// else the desktop's (`xdg-open`). Everything that opens the PDF goes
/// through here.
#[derive(Default)]
struct Viewer {
    /// The viewer started last, and when.
    started: Option<(std::process::Child, Instant)>,
    /// The live viewer.
    live: Option<crate::view::View>,
    /// The build gives the viewer its glyph origins itself (`watch
    /// --ssa`); else they are made from its `SyncTeX` file.
    glyph_origins: bool,
}

impl Viewer {
    /// The watch's viewer: the live one with `--view`, or by default where
    /// standard output is a terminal and `CI` is not set (the browser
    /// opened there, else its address shown); else the PDF's.
    fn start(opts: &Options, ren: &Renderer) -> Viewer {
        use std::io::IsTerminal;
        let interactive = std::io::stdout().is_terminal() && std::env::var_os("CI").is_none();
        if !opts.view.unwrap_or(interactive) {
            return Viewer::default();
        }
        let editor = match crate::editor::Editor::configured(opts.editor.as_deref()) {
            Ok(e) => e,
            Err(spec) => {
                ren.event(&format!(
                    "`{spec}` cannot open beside the watch: a double-click shows its source here"
                ));
                None
            }
        };
        // (kpathsea, for the viewer's text fonts)
        let host = crate::setup().host;
        match crate::view::View::start(host, ren.live(), editor) {
            Ok(v) => {
                let opened = interactive && crate::view::open_browser(v.url());
                let how = if opened {
                    ""
                } else {
                    " (open it in a browser)"
                };
                ren.status("Viewing", &format!("{}{how}", v.url()));
                Viewer {
                    started: None,
                    live: Some(v),
                    glyph_origins: false,
                }
            }
            Err(e) => {
                ren.warn("Viewer", &format!("not started: {e}"));
                Viewer::default()
            }
        }
    }

    /// A rebuild began.
    fn building(&self) {
        if let Some(v) = &self.live {
            v.building(true);
        }
    }

    /// What a build is doing (the live viewer shows it).
    fn progress(&self, p: &crate::events::Progress) {
        if let Some(v) = &self.live {
            v.progress(p);
        }
    }

    /// A build is in (`outputs`: its files; begun at `t`; `diagnostics`:
    /// what it reported).
    fn built(
        &self,
        outputs: &[(Vec<u8>, usize)],
        history: i32,
        t: Instant,
        diagnostics: &[partex_core::diag::Diagnostic],
    ) {
        if let Some(v) = &self.live {
            v.built(&crate::view::Built {
                diagnostics: diagnostics
                    .iter()
                    .filter(|d| d.severity != partex_core::diag::Severity::Note)
                    .map(crate::snippet::json)
                    .collect(),
                pdf: main_output(outputs)
                    .filter(|p| {
                        Path::new(p)
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
                    })
                    .map(PathBuf::from),
                history,
                ms: t.elapsed().as_secs_f64() * 1e3,
            });
            // (double-click and `phitex sync`: the glyphs placed by the
            // build's `SyncTeX` file, DESIGN 4.8)
            let synctex = outputs
                .iter()
                .map(|(n, _)| String::from_utf8_lossy(n).replace("//", "/"))
                .find(|n| n.ends_with(".synctex.gz") || n.ends_with(".synctex"));
            if !self.glyph_origins
                && let (Some(sync), Some(pdf)) = (synctex, Self::pdf(outputs))
            {
                v.set_origins_from_synctex(PathBuf::from(sync), pdf);
            }
        }
    }

    /// The build's glyph origins, as it recorded them (`watch --ssa`).
    fn set_origins(&self, o: crate::view::Origins) {
        if let Some(v) = &self.live {
            v.set_origins(Some(o));
        }
    }

    /// Pass `pass` of the build running wrote `outputs`, and passes may
    /// follow: its PDF is shown while they run.
    fn pass_written(&self, outputs: &[(Vec<u8>, usize)], pass: usize) {
        if let Some(v) = &self.live {
            v.set_pdf(Self::pdf(outputs));
            v.settling(pass);
        }
    }

    /// The PDF among `outputs`.
    fn pdf(outputs: &[(Vec<u8>, usize)]) -> Option<PathBuf> {
        main_output(outputs)
            .filter(|p| {
                Path::new(p)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            })
            .map(PathBuf::from)
    }

    /// A second press does not start a second viewer while the first
    /// still runs, nor within this long of it (`xdg-open` hands the PDF
    /// over and exits).
    const AGAIN_AFTER: Duration = Duration::from_secs(2);

    /// Show the PDF among `outputs`, unless the viewer started for it
    /// still runs (or has just started).
    fn open(&mut self, ren: &Renderer, target: &Target, outputs: &[(Vec<u8>, usize)]) {
        if let Some(v) = &self.live {
            crate::view::open_browser(v.url());
            ren.event(&format!("opening {}", v.url()));
            return;
        }
        let Some(pdf) = main_output(outputs) else {
            ren.event("no PDF yet");
            return;
        };
        let name = pdf.rsplit('/').next().unwrap_or(&pdf).to_owned();
        if let Some((child, at)) = &mut self.started {
            let running = matches!(child.try_wait(), Ok(None));
            if (running && target.viewer.is_some()) || at.elapsed() < Self::AGAIN_AFTER {
                ren.event(&format!("{name} is open already"));
                return;
            }
        }
        let program = target.viewer.as_deref().unwrap_or("xdg-open");
        match std::process::Command::new(program)
            .arg(&pdf)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.started = Some((child, Instant::now()));
                ren.event(&format!("opening {name}"));
            }
            Err(e) => ren.event(&format!("can't open {name} with {program}: {e}")),
        }
    }
}

/// `phitex watch`: build, then rebuild whenever an input changes (`q`
/// and Enter quits; on a terminal, keys).
#[allow(clippy::too_many_lines)]
fn watch(opts: &Options, target: &Target, st: Settings) -> ! {
    // (keys from the start: a key pressed while the first build runs)
    let keys = st.progress && term::keys_on();
    let ren = Renderer::for_watch(st, keys);
    let (tx, rx) = mpsc::channel();
    read_input(keys, ren.live(), tx.clone());
    if opts.diff.is_some() && (target.ssa || !target.machine) {
        let how = if target.ssa {
            "--ssa"
        } else {
            "--no-machine (or machine = false)"
        };
        fail(
            &format!("--diff needs the machine runtime, not {how}"),
            st.style,
        );
    }
    if target.ssa {
        ssawatch::watch(opts, target, &ren, &rx);
    }
    if target.machine {
        machine_watch(opts, target, &ren, &rx, &tx);
    }
    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
    ren.status("Compiling", &format!("{} ({})", target.file, target.engine));
    let mut sess = session(target, &ren);
    let mut viewer = Viewer::start(opts, &ren);
    ren.set_estimate(load_estimate());
    ren.start();
    let mut between = crate::Between::default();
    let t = Instant::now();
    let (reports, term, mut history) = crate::converge_saved(&mut sess, &mut between, &mut |p| {
        ren.progress(&p);
        viewer.progress(&p);
    });
    finish(target, &ren, &sess, &reports, &term, history, false, None);
    viewer.built(&sess.outputs(), history, t, &sess.diagnostics());
    BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
    save_estimate(&ren);
    if opts.open {
        viewer.open(&ren, target, &sess.outputs());
    }
    let stamps = |s: &crate::session::Session| -> Vec<(PathBuf, Option<SystemTime>)> {
        s.inputs()
            .into_iter()
            .map(|p| {
                let time = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                (p, time)
            })
            .collect()
    };
    let mut last = stamps(&sess);
    ren.watching(&target.file, main_output(&sess.outputs()).as_deref());
    let poll = poll_period(200);
    loop {
        let inputs = next_inputs(&rx, poll);
        if inputs.contains(&Input::Quit) {
            ren.close();
            term::exit(i32::from(history > 1));
        }
        let asked = inputs.contains(&Input::Rebuild);
        for &i in &inputs {
            answer(i, &ren, target, &sess.outputs(), &mut viewer);
        }
        if !inputs.is_empty() && !asked {
            continue;
        }
        let now = stamps(&sess);
        if now == last {
            if asked {
                ren.event("nothing changed");
            }
            continue;
        }
        let changed: Vec<String> = now
            .iter()
            .filter(|x| !last.contains(x))
            .map(|(p, _)| {
                let p = p.display().to_string();
                p.strip_prefix("./").map_or(p.clone(), str::to_owned)
            })
            .take(3)
            .collect();
        BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
        ren.start_rebuild();
        viewer.building();
        let t = Instant::now();
        let mut edited = saved_since(&sess, now);
        let (reports, term, h) = crate::serve_watched(
            &mut sess,
            Some(history),
            true,
            &mut between,
            &mut |p| {
                ren.progress(&p);
                viewer.progress(&p);
            },
            &mut edited,
        );
        BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
        history = h;
        let rebuild = Some(Rebuild { changed });
        finish(target, &ren, &sess, &reports, &term, h, false, rebuild);
        viewer.built(&sess.outputs(), h, t, &sess.diagnostics());
        last = stamps(&sess);
        ren.watching(&target.file, main_output(&sess.outputs()).as_deref());
        if INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
            ren.close();
            term::exit(i32::from(history > 1));
        }
    }
}

/// What a watch asks between the passes of a rebuild of `sess` (inputs
/// with their stamps `now`, as it began): the files saved since, not the
/// job's own (a save while the job's own files settle supersedes the
/// passes left), each named once a save.
fn saved_since(
    sess: &crate::session::Session,
    now: Vec<(PathBuf, Option<SystemTime>)>,
) -> impl FnMut() -> Vec<String> + use<> {
    let outputs: Vec<PathBuf> = sess
        .outputs()
        .iter()
        .map(|(n, _)| crate::native::path(n))
        .collect();
    let mut seen: Vec<(PathBuf, Option<SystemTime>)> = now
        .into_iter()
        .filter(|(p, _)| !outputs.iter().any(|o| same_file(o, p)))
        .collect();
    move || {
        let mut saved = Vec::new();
        for (p, time) in &mut seen {
            let t = std::fs::metadata(&*p).and_then(|m| m.modified()).ok();
            if t != *time {
                *time = t;
                let p = p.display().to_string();
                saved.push(p.strip_prefix("./").map_or(p.clone(), str::to_owned));
            }
        }
        saved
    }
}

/// Whether `a` and `b` name the same file (`./x.aux` and `x.aux`).
fn same_file(a: &Path, b: &Path) -> bool {
    let strip = |p: &Path| p.strip_prefix(".").unwrap_or(p).to_path_buf();
    strip(a) == strip(b)
}

/// Render the end of a machine-mode build (as [`finish`] does a
/// session's) and record it: the exit status.
fn machine_finish(
    t: &Target,
    r: &Renderer,
    out: &crate::machinehost::Outcome,
    rebuild: Option<Rebuild>,
    watching: bool,
) -> i32 {
    let summary = crate::warnings::summarize(&out.diagnostics);
    let failed = out.history > 1;
    let output = main_output(&out.outputs);
    r.finish(&End {
        file: &t.file,
        summary: &summary,
        term: &out.term,
        bytes: output_bytes(&out.outputs, output.as_deref()),
        output,
        failed,
        checked: false,
        rebuild,
    });
    if r.verbose() {
        for line in &out.reports {
            // (`watch --ssa`'s lines are the SSA runtime's)
            if let Some(line) = line.strip_prefix("phitex: ssa: ") {
                r.status("SSA", line);
                continue;
            }
            let line = line
                .strip_prefix("phitex: machine: ")
                .or_else(|| line.strip_prefix("phitex: "))
                .unwrap_or(line);
            r.status("Machine", line);
        }
    }
    if !out.unsettled.is_empty() {
        // (as latexmk's "Rerun": the job's own files did not reach their
        // fixpoint; a watch rebuilds again only for an edit)
        r.warn(
            "Unsettled",
            &format!(
                "{} still changed after {} passes{}",
                out.unsettled.join(", "),
                crate::machinehost::PASSES,
                if watching {
                    "; waiting for an edit"
                } else {
                    ""
                }
            ),
        );
    }
    record(t, &out.reports, &summary, &out.outputs);
    if !failed {
        copy_final_pdf(t, r, &out.outputs);
    }
    i32::from(failed)
}

/// Stop a machine-mode watch: the build saved as it is, then the exit.
fn machine_quit(ren: &Renderer, w: &mut crate::machinehost::Watch, history: i32) -> ! {
    ren.progress(&crate::events::Progress::Phase(
        crate::events::Phase::Saving,
    ));
    w.finish_saving();
    ren.idle();
    store_hint(ren);
    ren.close();
    term::exit(i32::from(history > 1));
}

/// `phitex watch` in machine mode (DESIGN.md §6.1, §7.0): the build is
/// recorded as regions, and an edit re-runs only the regions whose reads
/// it changed. `ren` and `rx` are the watch's, reading keys already.
#[allow(clippy::too_many_lines)]
fn machine_watch(
    opts: &Options,
    target: &Target,
    ren: &Renderer,
    rx: &mpsc::Receiver<Input>,
    tx: &mpsc::Sender<Input>,
) -> ! {
    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
    ren.status("Compiling", &format!("{} ({})", target.file, target.engine));
    let formats = ensure_format(&target.engine, Some(ren));
    let mut diff = diffmode::Diff::new(target, opts.diff.as_deref(), formats.clone())
        .unwrap_or_else(|e| fail(&e, ren.style()));
    crate::set_args(target.engine_args("nonstopmode"));
    let mut job = crate::setup();
    job.host.formats = formats;
    job.host.notes = true;
    crate::make_output_dir(&job.host);
    let mut viewer = Viewer::start(opts, ren);
    if let Some(v) = &viewer.live {
        let tx = tx.clone();
        v.on_ask(Box::new(move || {
            let _ = tx.send(Input::Viewer);
        }));
    }
    ren.set_estimate(load_estimate());
    ren.start();
    let mut between = crate::Between::default();
    let t = Instant::now();
    let (opened, out) = crate::machinehost::Watch::open(
        job.host,
        job.params,
        job.command_line.as_bytes(),
        &mut between,
        &mut |p| {
            ren.progress(&p);
            viewer.progress(&p);
        },
        true,
    );
    let mut history = out.history;
    let mut outputs = out.outputs.clone();
    machine_finish(target, ren, &out, None, true);
    viewer.built(&outputs, history, t, &out.diagnostics);
    save_estimate(ren);
    if opts.open {
        viewer.open(ren, target, &out.outputs);
    }
    // (the document's last build, shown again when the diff is not)
    let mut last = (out, t);
    // (after a restart with nothing changed: the saved build, loading
    // meanwhile, and what changed since the look)
    let t = Instant::now();
    let (mut w, since) = opened.into_watch(&mut between, &mut |p| {
        ren.progress(&p);
        viewer.progress(&p);
    });
    if let Some(out) = since {
        history = out.history;
        outputs.clone_from(&out.outputs);
        let changed = w.last_changes().to_vec();
        machine_finish(target, ren, &out, Some(Rebuild { changed }), true);
        viewer.built(&outputs, history, t, &out.diagnostics);
        last = (out, t);
    }
    BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
    w.idle();
    ren.watching(&target.file, main_output(&outputs).as_deref());
    if opts.diff.is_some() {
        diff.key(Input::Diff, target, ren, &viewer, (&last.0, last.1));
    }
    let poll = poll_period(50);
    loop {
        let inputs = next_inputs(rx, poll);
        if inputs.contains(&Input::Quit) {
            machine_quit(ren, &mut w, history);
        }
        let asked = inputs.contains(&Input::Rebuild);
        for &i in &inputs {
            if !diff.key(i, target, ren, &viewer, (&last.0, last.1)) {
                answer(i, ren, target, &outputs, &mut viewer);
            }
        }
        // (only the job shown rebuilds: the document's when it is shown
        // again)
        if diff.shown {
            if asked || inputs.is_empty() {
                diff.poll(target, ren, &viewer, asked);
            }
            continue;
        }
        if !asked && (!inputs.is_empty() || !w.changed()) {
            continue;
        }
        BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
        ren.start_rebuild();
        viewer.building();
        let t = Instant::now();
        let out = w.rebuild(&mut between, &mut |p| {
            ren.progress(&p);
            viewer.progress(&p);
        });
        BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
        let Some(out) = out else {
            ren.idle();
            if asked {
                ren.event("nothing changed");
            }
            continue;
        };
        history = out.history;
        outputs.clone_from(&out.outputs);
        let changed = w.last_changes().to_vec();
        machine_finish(target, ren, &out, Some(Rebuild { changed }), true);
        viewer.built(&outputs, history, t, &out.diagnostics);
        last = (out, t);
        w.idle();
        ren.watching(&target.file, main_output(&outputs).as_deref());
        if INTERRUPTED.load(std::sync::atomic::Ordering::Relaxed) {
            machine_quit(ren, &mut w, history);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_dirs() {
        let (cwd, root) = (Path::new("/home/u/paper/sec"), Path::new("/home/u/paper"));
        let dir = |c| copy_dir(&c, cwd, root);
        assert_eq!(dir(config::CopyPdf::Off), None);
        assert_eq!(dir(config::CopyPdf::Invocation), Some(cwd.to_path_buf()));
        assert_eq!(
            dir(config::CopyPdf::Dir("out".into())),
            Some(PathBuf::from("/home/u/paper/out"))
        );
        assert_eq!(
            dir(config::CopyPdf::Dir("/srv/pdf".into())),
            Some(PathBuf::from("/srv/pdf"))
        );
    }

    #[test]
    fn copies_pdf_atomically() {
        let base = std::env::temp_dir().join(format!("partex-copy-pdf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, dst) = (base.join("build"), base.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        let pdf = src.join("paper.pdf");
        std::fs::write(&pdf, b"%PDF-1.5 one").unwrap();
        // made if missing; the copy replaces an older one
        assert_eq!(copy_pdf(&pdf, &dst).unwrap(), Some(dst.join("paper.pdf")));
        std::fs::write(&pdf, b"%PDF-1.5 two").unwrap();
        assert_eq!(copy_pdf(&pdf, &dst).unwrap(), Some(dst.join("paper.pdf")));
        assert_eq!(
            std::fs::read(dst.join("paper.pdf")).unwrap(),
            b"%PDF-1.5 two"
        );
        // no temporary file is left behind
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 1);
        // into its own directory: nothing to do
        assert_eq!(copy_pdf(&pdf, &src).unwrap(), None);
        assert_eq!(std::fs::read(&pdf).unwrap(), b"%PDF-1.5 two");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn escape_sequences() {
        assert_eq!(escape(b""), (Some(Input::Esc), 0));
        assert_eq!(escape(b"[A"), (Some(Input::Up), 2));
        assert_eq!(escape(b"OBx"), (Some(Input::Down), 2));
        // (a function key: nothing, all of it read)
        assert_eq!(escape(b"[15~q"), (None, 4));
        assert_eq!(escape(b"x"), (Some(Input::Esc), 0));
        assert_eq!(line_input(" d "), Some(Input::Diff));
        assert_eq!(line_input("N"), Some(Input::Prev));
        assert_eq!(line_input("x"), None);
    }

    #[test]
    fn steps_through_a_list_count_each_press() {
        let (tx, rx) = mpsc::channel();
        for i in [Input::Down, Input::Down, Input::Diff, Input::Diff] {
            tx.send(i).unwrap();
        }
        let got = next_inputs(&rx, Duration::from_millis(10));
        assert_eq!(got, vec![Input::Down, Input::Down, Input::Diff]);
    }

    #[test]
    fn keys_pressed_during_a_build_are_answered_once() {
        let (tx, rx) = mpsc::channel();
        for i in [Input::Open, Input::Open, Input::Problems, Input::Open] {
            tx.send(i).unwrap();
        }
        let poll = Duration::from_millis(10);
        assert_eq!(next_inputs(&rx, poll), vec![Input::Open, Input::Problems]);
        assert_eq!(next_inputs(&rx, poll), Vec::new());
    }

    #[test]
    fn the_viewer_starts_once() {
        let st = Settings {
            style: Style {
                color: false,
                unicode: true,
                links: false,
            },
            progress: false,
            verbose: 0,
            quiet: false,
        };
        let (ren, buf) = Renderer::captured(st, true);
        let target = Target {
            file: "paper.tex".into(),
            engine: "pdflatex".into(),
            output_dir: None,
            shell_escape: None,
            viewer: Some("true".into()),
            copy_pdf: None,
            machine: true,
            ssa: false,
            synctex: true,
        };
        let outputs = vec![(b"out/paper.pdf".to_vec(), 100)];
        let mut viewer = Viewer::default();
        viewer.open(&ren, &target, &outputs);
        viewer.open(&ren, &target, &outputs);
        let out = String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
        assert_eq!(out.matches("opening paper.pdf").count(), 1, "{out}");
        assert_eq!(out.matches("paper.pdf is open already").count(), 1, "{out}");
    }

    #[test]
    fn clean_helpers() {
        assert_eq!(and_list(&[]), "");
        assert_eq!(and_list(&["a".into()]), "a");
        assert_eq!(
            and_list(&[
                "2 files".into(),
                "the saved session".into(),
                "the saved build".into()
            ]),
            "2 files, the saved session and the saved build"
        );
        let base = std::env::temp_dir().join(format!("phitex-bytes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/b")).unwrap();
        std::fs::write(base.join("x"), [0; 100]).unwrap();
        std::fs::write(base.join("a/b/y"), [0; 23]).unwrap();
        assert_eq!(bytes_under(&base), 123);
        assert_eq!(bytes_under(&base.join("none")), 0);
        std::fs::remove_dir_all(&base).unwrap();
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let p = Path::new(&home).join(".cache/phitex/store");
            assert_eq!(tilde(&p), "~/.cache/phitex/store");
        }
        assert_eq!(tilde(Path::new("/srv/store")), "/srv/store");
    }

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn options() {
        let o = parse(&v(&[
            "build",
            "-vv",
            "--color=never",
            "-o",
            "out",
            "paper.tex",
            "--message-format",
            "json",
        ]))
        .unwrap();
        assert_eq!(o.command, Command::Build);
        assert_eq!(o.verbose, 2);
        assert_eq!(o.color, ColorChoice::Never);
        assert_eq!(o.output_dir.as_deref(), Some("out"));
        assert_eq!(o.file.as_deref(), Some("paper.tex"));
        assert!(o.json);
        assert!(parse(&v(&["build", "--frobnicate"])).is_err());
        let o = parse(&v(&["watch", "--copy-pdf", "paper.tex"])).unwrap();
        assert_eq!(o.copy_pdf, Some(config::CopyPdf::Invocation));
        assert_eq!(o.file.as_deref(), Some("paper.tex"));
        let o = parse(&v(&["build", "--copy-pdf=../pdfs"])).unwrap();
        assert_eq!(o.copy_pdf, Some(config::CopyPdf::Dir("../pdfs".into())));
        assert!(parse(&v(&["build", "--copy-pdf="])).is_err());
        assert!(parse(&v(&["watch", "--ssa", "paper.tex"])).unwrap().ssa);
        assert!(
            parse(&v(&["-ini"]))
                .unwrap_err()
                .contains("--compat=tex -ini")
        );
    }

    #[test]
    fn engine_command_line() {
        let t = Target {
            file: "paper.tex".into(),
            engine: "pdflatex".into(),
            output_dir: Some("out".into()),
            shell_escape: None,
            viewer: None,
            copy_pdf: None,
            machine: true,
            ssa: false,
            synctex: true,
        };
        assert_eq!(
            t.engine_args("nonstopmode"),
            v(&[
                "pdflatex",
                "-interaction=nonstopmode",
                "-synctex=1",
                "-output-directory=out",
                "paper.tex"
            ])
        );
        assert!(
            !t.engine_args_with("nonstopmode", false)
                .contains(&"-synctex=1".to_owned())
        );
        assert_eq!(t.job(), "paper");
        assert_eq!(engine_name("XeLaTeX").unwrap(), "xelatex");
        assert!(
            engine_name("LuaLaTeX")
                .unwrap_err()
                .contains("not supported")
        );
    }
}
