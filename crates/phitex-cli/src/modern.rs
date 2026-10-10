//! The modern command line: `phitex build`, `phitex watch`, … What to
//! build comes from the command line, `phitex.toml` and the main file's
//! magic comments (`config.rs`); the build is a session on phitex-flow
//! (`session.rs`), to the cross-run fixpoint, BibTeX and makeindex
//! included.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use crate::config;
use crate::session::{self, Job, Report, Session};
use crate::tex::{self, Output};

const USAGE: &str = "\
PhiTeX: TeX documents as dataflow graphs (phitex-flow)

usage: phitex <command> [options] [file.tex]

commands:
  build    build the document to its fixpoint (BibTeX and makeindex included)
  watch    build, then rebuild whenever an input changes (an edit of the
           graph), the pages live in the browser; keys on standard input:
           r rebuild, q quit
  check    build without writing the outputs
  clean    remove the files a build writes (clean --all: the cache of
           formats and TeX Live's links too; no file needed)
  sync     sync FILE:LINE[:COL]: the watch's viewer shows that line
  outline  the document's structure, read from its source without TeX

options:
  -o, --output-dir DIR     where the outputs go (TeX's -output-directory)
      --engine NAME        pdflatex (default for LaTeX), latex, pdftex (default
                           otherwise), etex or tex
      --output-format F    dvi or pdf (latex/pdflatex, etex/pdftex)
      --ini [--etex]       the file is the whole job: no format (TeX's -ini)
  -j, --jobs N             threads (default: all cores)
      --interaction MODE   batchmode, nonstopmode (default), scrollmode
  -v, --verbose            each run, and each warning's first lines
  -q, --quiet              only problems and the result
      --open               watch: open the viewer in the browser
      --view, --no-view    watch: serve the live viewer (default: on a terminal)
      --editor CMD         watch: where a double-click on a page opens its source
      --copy-pdf[=DIR]     copy the PDF after each successful build
      --synctex            write <job>.synctex.gz (not yet on phitex-flow)
      --recorder           write <job>.fls
  -V, --version
  -h, --help

Without a file, `phitex.toml` (here or above) names it:
    main = \"paper.tex\"   engine = \"pdflatex\"   output-dir = \"out\"
    copy-pdf = true   (or a directory, relative to phitex.toml's)
A `% !TEX program = …` or `% !TEX root = …` comment in the file is honoured.

For TeX's own command line run phitex as `pdflatex`, `latex`, `pdftex`,
`etex` or `tex`, or `phitex --compat=NAME …`.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Build,
    Watch,
    Check,
    Clean,
    Sync,
}

/// The modern command line's options.
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
    pub interaction: Option<i32>,
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

fn parse(args: &[String]) -> Result<Options, String> {
    let command = match args.first().map(String::as_str) {
        Some("build" | "b") => Command::Build,
        Some("watch" | "w") => Command::Watch,
        Some("check" | "c") => Command::Check,
        Some("clean") => Command::Clean,
        Some("sync") => Command::Sync,
        Some("-h" | "--help" | "help") | None => return Err(String::new()),
        Some(a) if a.starts_with('-') || a.starts_with('&') || a.starts_with('\\') => {
            return Err(format!(
                "`{a}` is a TeX command-line argument; use `phitex --compat=pdftex {a} …` \
                 (or run phitex as pdftex, pdflatex, …) for TeX's command line"
            ));
        }
        Some(c) => return Err(format!("unknown command `{c}`")),
    };
    let mut o = Options { command: Some(command), ..Options::default() };
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
            "-o" | "--output-dir" | "--output-directory" | "--out-dir" => o.output_dir = Some(value()?),
            "--engine" | "--program" => o.engine = Some(value()?),
            "--output-format" | "-output-format" => {
                o.output = Some(match value()?.as_str() {
                    "dvi" => Output::Dvi,
                    "pdf" => Output::Pdf,
                    v => return Err(format!("--output-format: `{v}` is not dvi or pdf")),
                });
            }
            "--ini" => o.ini = true,
            "--etex" => o.etex = true,
            "-j" | "--jobs" | "--threads" => {
                o.threads = Some(value()?.parse().map_err(|_| "--jobs needs a number".to_owned())?);
            }
            "--interaction" => {
                let v = value()?;
                o.interaction = Some(
                    crate::compat::interaction(&v).ok_or_else(|| format!("--interaction: `{v}` is not a mode"))?,
                );
            }
            "-v" | "--verbose" => o.verbose += 1,
            "-vv" => o.verbose += 2,
            "-q" | "--quiet" => o.quiet = true,
            "--open" => o.open = true,
            "--view" => o.view = Some(true),
            "--no-view" => o.view = Some(false),
            "--editor" => o.editor = Some(value()?),
            "--synctex" => o.synctex = Some(true),
            "--no-synctex" => o.synctex = Some(false),
            "--recorder" => o.recorder = true,
            // (the value only inline: `--copy-pdf paper.tex` names the file)
            "--copy-pdf" => {
                o.copy_pdf = Some(match &inline {
                    Some(d) if d.is_empty() => return Err("`--copy-pdf=` needs a directory".into()),
                    Some(d) => config::CopyPdf::Dir(d.clone()),
                    None => config::CopyPdf::Invocation,
                });
            }
            "--no-copy-pdf" => o.copy_pdf = Some(config::CopyPdf::Off),
            "--all" if command == Command::Clean => o.all = true,
            "-h" | "--help" => return Err(String::new()),
            f if f.starts_with('-') && f.len() > 1 => return Err(format!("unknown option `{f}`")),
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
pub struct Target {
    /// The main file, as TeX is given it (from the directory the build
    /// runs in).
    pub file: String,
    pub engine: tex::Engine,
    pub output_dir: Option<String>,
    /// Where to copy the PDF after a successful build.
    pub copy_pdf: Option<PathBuf>,
    pub synctex: bool,
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
fn resolve(o: &Options) -> Result<Target, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let found = config::find(&cwd)?;
    let (root, cfg, configured) = match (&o.file, found) {
        (None, Some((root, cfg))) => {
            std::env::set_current_dir(&root).map_err(|e| format!("can't enter {}: {e}", root.display()))?;
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
    let mut file = o
        .file
        .clone()
        .or_else(|| cfg.main.clone())
        .ok_or("no file given, and no phitex.toml names one (`main = \"paper.tex\"`)".to_owned())?;
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
    let mut engine = match o.engine.clone().or(cfg.engine).or(program) {
        Some(e) => tex::engine(&e)?,
        None => tex::engine(config::default_engine(&text))?,
    };
    if let Some(out) = o.output {
        engine = engine.with_output(out)?;
    }
    let mut output_dir = o.output_dir.clone().or(cfg.output_dir);
    if !configured && let Some(dir) = Path::new(&file).parent().filter(|d| !d.as_os_str().is_empty()) {
        let dir = dir.to_path_buf();
        output_dir = output_dir.map(|d| cwd.join(d).to_string_lossy().into_owned());
        std::env::set_current_dir(&dir).map_err(|e| format!("can't enter {}: {e}", dir.display()))?;
        file = Path::new(&file).file_name().map_or(file.clone(), |f| f.to_string_lossy().into_owned());
    }
    Ok(Target {
        file,
        engine,
        output_dir,
        copy_pdf,
        synctex: o.synctex.or(cfg.synctex).unwrap_or(false),
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
        out_dir: t.output_dir.as_ref().map_or_else(|| PathBuf::from("."), PathBuf::from),
        interaction: o.interaction.unwrap_or(1),
        threads: o.threads.unwrap_or_else(crate::default_threads),
        fixpoint: true,
        tools: true,
        synctex: t.synctex,
        recorder: o.recorder,
        pdf_date: session::pdf_date(),
    })
}

/// Colours on standard error, unless it is no terminal or `NO_COLOR`.
pub struct Style {
    color: bool,
}

impl Style {
    pub fn detect() -> Style {
        let force = std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0");
        let no = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Style { color: !no && (force || std::io::stderr().is_terminal()) }
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_owned() }
    }

    pub fn red(&self, s: &str) -> String {
        self.paint("1;31", s)
    }

    pub fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }

    pub fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }

    pub fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
}

/// Milliseconds of a duration, for the terminal.
pub fn ms(d: std::time::Duration) -> String {
    let m = d.as_secs_f64() * 1e3;
    if m >= 1000.0 { format!("{:.2} s", m / 1e3) } else { format!("{m:.1} ms") }
}

/// What a build did, for the terminal: each run (verbose), the tools, the
/// result, the errors, the warnings.
pub fn report(t: &Target, s: &Session, r: &Report, wrote: Option<&Path>, o: &Options, st: &Style) {
    let job = &s.job;
    if o.verbose > 0 {
        for (k, run) in r.runs.iter().enumerate() {
            eprintln!(
                "{}",
                st.dim(&format!(
                    "  run {}: {} ({}): {} of {} commands, {} of {} pieces ran, {} pages ({} kept)",
                    k + 1,
                    ms(run.wall),
                    run.why,
                    run.commands_run,
                    run.commands,
                    run.pieces_ran,
                    run.pieces,
                    run.pages,
                    run.pages_reused
                ))
            );
        }
    }
    if !o.quiet {
        for n in &r.notes {
            eprintln!("{}", st.dim(&format!("  {n}")));
        }
    }
    let log = job.out_dir.join(format!("{}.log", job.jobname));
    for e in r.out.errors.iter().take(if o.verbose > 0 { 100 } else { 10 }) {
        eprintln!("{} {}", st.red("!"), crate::format::bare(e));
    }
    if r.out.errors.len() > 10 && o.verbose == 0 {
        eprintln!("  … {} errors in all (-v shows more; the log: {})", r.out.errors.len(), log.display());
    }
    if !r.unsettled.is_empty() {
        eprintln!(
            "{}: the written files did not settle in {} runs: {} still changed (oscillating?)",
            st.yellow("warning"),
            r.runs.len(),
            r.unsettled.join(", ")
        );
    }
    let warn = crate::warnings::lines(&r.log_text(), o.verbose > 0);
    if !warn.is_empty() && !o.quiet {
        eprintln!("{}: {} ({})", st.yellow("warnings"), warn.iter().filter(|l| !l.starts_with(' ')).cloned().collect::<Vec<_>>().join(", "), log.display());
        for l in warn.iter().filter(|l| l.starts_with(' ')) {
            eprintln!("{}", st.dim(l));
        }
    }
    let what = match (wrote, r.main_output()) {
        (Some(p), Some((_, b))) => {
            format!("{}: {} page{}, {} bytes", p.display(), r.pages(), if r.pages() == 1 { "" } else { "s" }, b.len())
        }
        (None, Some(_)) => format!("{} pages (not written)", r.pages()),
        _ => "no pages of output".to_owned(),
    };
    let runs = r.runs.len();
    let line = format!(
        "{what}, in {} ({} run{}{})",
        ms(r.wall),
        runs,
        if runs == 1 { "" } else { "s" },
        if job.ini { String::new() } else { format!(", {}", t.engine.name) }
    );
    if r.out.errors.is_empty() {
        eprintln!("{}", st.green(&line));
    } else {
        eprintln!("{} ({} error{})", st.red(&line), r.out.errors.len(), if r.out.errors.len() == 1 { "" } else { "s" });
    }
}

/// Copy the written PDF into `dir`.
fn copy_pdf(main: &Path, dir: &Path, st: &Style) {
    if main.extension().is_none_or(|e| e != "pdf") {
        return;
    }
    let Some(name) = main.file_name() else { return };
    let to = dir.join(name);
    if let Err(e) = std::fs::copy(main, &to) {
        eprintln!("{}: can't copy the PDF to {}: {e}", st.yellow("warning"), to.display());
    }
}

/// `phitex build` / `phitex check`: one session to the fixpoint. The exit
/// status.
fn build(t: &Target, o: &Options, st: &Style, write: bool) -> i32 {
    let job = match job(t, o) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("{}: {e}", st.red("error"));
            return 2;
        }
    };
    for n in session::waiting_notes(&job) {
        eprintln!("{}: {n}", st.yellow("note"));
    }
    let quiet = o.quiet;
    let mut s = match Session::new(job, &mut |n: &str| {
        if !quiet {
            eprintln!("{}", st.dim(&format!("phitex: {n}")));
        }
    }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", st.red("error"));
            return 1;
        }
    };
    let r = s.build();
    let mut main = None;
    if write {
        match s.write(&r) {
            Ok(w) => main = w.main,
            Err(e) => {
                eprintln!("{}: {e}", st.red("error"));
                return 1;
            }
        }
    }
    report(t, &s, &r, main.as_deref(), o, st);
    if let (Some(m), Some(d), true) = (&main, &t.copy_pdf, r.out.errors.is_empty()) {
        copy_pdf(m, d, st);
    }
    i32::from(!r.out.errors.is_empty())
}

/// The files a build of job `job` writes (`clean`).
const CLEAN: &[&str] = &[
    "pdf", "dvi", "xdv", "log", "fls", "synctex.gz", "aux", "toc", "lof", "lot", "out", "bbl", "blg", "idx",
    "ind", "ilg", "nav", "snm", "vrb", "bcf", "run.xml", "glo", "gls", "glg", "loa", "thm",
];

fn clean(t: &Target, st: &Style) -> i32 {
    let dir = t.output_dir.as_ref().map_or_else(|| PathBuf::from("."), PathBuf::from);
    let mut n = 0;
    for ext in CLEAN {
        let p = dir.join(format!("{}.{ext}", t.job()));
        if std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    eprintln!("{}", st.green(&format!("removed {n} file{}", if n == 1 { "" } else { "s" })));
    0
}

fn clean_all(st: &Style) -> ! {
    let d = tex::cache_dir();
    match std::fs::remove_dir_all(&d) {
        Ok(()) => eprintln!("{}", st.green(&format!("removed {}", d.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => eprintln!("nothing cached ({})", d.display()),
        Err(e) => {
            eprintln!("{}: {}: {e}", st.red("error"), d.display());
            std::process::exit(1);
        }
    }
    std::process::exit(0);
}

/// Run the modern command line with `args`.
pub fn main(args: &[String]) -> ! {
    if matches!(args.first().map(String::as_str), Some("-V" | "--version")) {
        println!("PhiTeX {} (phitex-flow)", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    if args.first().is_some_and(|a| a == "outline") {
        crate::outline::main(&args[1..]);
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
    let st = Style::detect();
    if o.all {
        clean_all(&st);
    }
    let command = o.command.unwrap_or(Command::Build);
    if command == Command::Sync {
        let Some(place) = o.file.as_deref() else {
            eprintln!("{}: sync: give FILE:LINE[:COL]", st.red("error"));
            std::process::exit(2);
        };
        std::process::exit(crate::view::sync_command(place));
    }
    let t = resolve(&o).unwrap_or_else(|e| {
        eprintln!("{}: {e}", st.red("error"));
        std::process::exit(2);
    });
    match command {
        Command::Build => std::process::exit(build(&t, &o, &st, true)),
        Command::Check => std::process::exit(build(&t, &o, &st, false)),
        Command::Clean => std::process::exit(clean(&t, &st)),
        Command::Watch => crate::watch::main(&t, &o, &st),
        Command::Sync => unreachable!("answered above"),
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
        assert!(parse(&v(&["-ini", "x"])).unwrap_err().contains("TeX command-line"));
        assert!(parse(&v(&["build", "--frob"])).is_err());
        assert!(parse(&v(&["watch", "--no-view", "--ini", "--etex", "a.tex"])).unwrap().ini);
    }
}
