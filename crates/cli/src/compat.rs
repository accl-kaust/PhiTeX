//! phitex under an engine's name (`tex`, `etex`, `pdftex`, `latex`,
//! `pdflatex`, as links to it), or `phitex --compat=NAME …`: web2c's
//! command line and TeX's terminal output, one run as TeX makes one, for
//! latexmk, editors and scripts. The run is a flow build like any other
//! (`session.rs`), without the fixpoint.

use std::path::{Path, PathBuf};

use crate::session::{self, Job, Session};
use crate::tex::{self, Output};

/// The names that are the modern command line, not an engine's.
const MODERN: &[&str] = &["phitex", "phitex"];

/// What an invocation is.
#[derive(Debug, PartialEq, Eq)]
pub enum Selection {
    /// An engine's command line: the program name and the arguments.
    Compat { progname: String, args: Vec<String> },
    /// The modern command line.
    Modern(Vec<String>),
}

/// How phitex was invoked: `argv0`, the arguments after it, and
/// `PHITEX_COMPAT`.
#[must_use]
pub fn select(argv0: &str, mut args: Vec<String>, env: Option<&str>) -> Selection {
    let stem = Path::new(argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("phitex");
    if !MODERN.contains(&stem) {
        return Selection::Compat {
            progname: stem.to_owned(),
            args,
        };
    }
    if let Some(first) = args.first() {
        let opt = first.strip_prefix("--").or_else(|| first.strip_prefix('-'));
        if let Some(name) = opt.and_then(|o| o.strip_prefix("compat=")) {
            let progname = name.to_owned();
            args.remove(0);
            return Selection::Compat { progname, args };
        }
        if matches!(opt, Some("compat")) && args.len() > 1 {
            let progname = args[1].clone();
            args.drain(..2);
            return Selection::Compat { progname, args };
        }
    }
    match env.filter(|e| !e.is_empty()) {
        Some(name) => Selection::Compat {
            progname: name.to_owned(),
            args,
        },
        None => Selection::Modern(args),
    }
}

/// The options that take a value, which web2c also accepts as the next
/// argument.
const VALUE_OPTIONS: &[&str] = &[
    "default-translate-file",
    "fmt",
    "interaction",
    "jobname",
    "output-comment",
    "output-directory",
    "output-format",
    "progname",
    "synctex",
    "translate-file",
    "threads",
];

/// The interaction mode a name stands for.
pub fn interaction(name: &str) -> Option<i32> {
    Some(match name {
        "batchmode" => 0,
        "nonstopmode" => 1,
        "scrollmode" => 2,
        "errorstopmode" => 3,
        _ => return None,
    })
}

fn usage(progname: &str) -> ! {
    eprintln!(
        "usage: {progname} [-ini] [-etex] [-interaction=MODE] [-jobname=NAME] [-output-directory=DIR] \
         [-output-format=dvi|pdf] [-fmt=NAME] [-recorder] [-synctex=N] [-threads=N] [&FORMAT] FILE"
    );
    std::process::exit(2);
}

/// Run as the engine `progname` with `args`; never returns.
#[allow(clippy::too_many_lines, reason = "one option an arm")]
pub fn main(progname: &str, args: &[String]) -> ! {
    let mut ini = progname == "initex";
    let mut shell_escape = false;
    let mut etex = false;
    let mut jobname = None;
    let mut out_dir = std::env::var("TEXMF_OUTPUT_DIRECTORY")
        .ok()
        .filter(|d| !d.is_empty());
    let mut output = None;
    let mut fmt = None;
    let mut recorder = false;
    let mut synctex = false;
    let mut threads = None;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !rest.is_empty() {
            rest.push(a.clone());
            continue;
        }
        let Some(opt) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')) else {
            rest.push(a.clone());
            continue;
        };
        let (name, value) = match opt.split_once('=') {
            Some((n, v)) => (n, Some(v.to_owned())),
            None if VALUE_OPTIONS.contains(&opt) => (
                opt,
                Some(it.next().cloned().unwrap_or_else(|| usage(progname))),
            ),
            None => (opt, None),
        };
        let v = || value.clone().unwrap_or_default();
        match name {
            "version" => {
                println!("phitex {} as {progname}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "help" => usage(progname),
            "ini" => ini = true,
            "etex" => etex = true,
            "interaction" => {
                // (every job runs to its end: the mode is checked, not kept)
                interaction(&v()).unwrap_or_else(|| usage(progname));
            }
            "jobname" => jobname = Some(v()),
            "output-directory" => out_dir = Some(v()),
            "output-format" => {
                output = Some(match v().as_str() {
                    "dvi" => Output::Dvi,
                    "pdf" => Output::Pdf,
                    _ => usage(progname),
                });
            }
            "fmt" | "progname" => fmt = Some(v()),
            "recorder" => recorder = true,
            "synctex" => synctex = v().trim() != "0",
            "threads" | "j" => threads = v().parse().ok(),
            // (accepted, and as TeX Live's defaults are: no effect here)
            "file-line-error"
            | "no-file-line-error"
            | "halt-on-error"
            | "8bit"
            | "translate-file"
            | "default-translate-file"
            | "parse-first-line"
            | "no-parse-first-line"
            | "output-comment"
            | "shell-restricted"
            | "src-specials"
            | "cnf-line" => {}
            "shell-escape" => shell_escape = true,
            "no-shell-escape" => shell_escape = false,
            "draftmode" | "mktex" | "no-mktex" | "kpathsea-debug" | "ipc" | "ipc-start" | "enc"
            | "mltex" => {
                eprintln!("{progname}: -{name} is not supported by phitex");
                std::process::exit(2);
            }
            _ => usage(progname),
        }
    }
    // (`&name` on the first line names the format)
    if let Some(f) = rest.first().and_then(|a| a.strip_prefix('&')) {
        fmt = Some(f.to_owned());
        rest.remove(0);
    }
    let Some(first) = rest.first().cloned() else {
        eprintln!("{progname}: phitex needs a file name on the command line (no terminal input)");
        std::process::exit(2);
    };
    if first.starts_with('\\') || rest.len() > 1 {
        eprintln!(
            "{progname}: a first line of TeX commands (`{}`) is not supported: name the main file",
            rest.join(" ")
        );
        std::process::exit(2);
    }
    // (`*name`: e-TeX's extended mode, as fmtutil gives it)
    let (file, star) = match first.strip_prefix('*') {
        Some(f) => (f.to_owned(), true),
        None => (first, false),
    };
    let name = fmt.as_deref().unwrap_or(progname);
    let engine = tex::engine(name)
        .and_then(|e| match output {
            Some(o) if !ini => e.with_output(o),
            _ => Ok(e),
        })
        .unwrap_or_else(|e| {
            eprintln!("{progname}: {e}");
            std::process::exit(2);
        });
    let path = find_main(&file).unwrap_or_else(|| {
        eprintln!("! I can't find file `{file}'.");
        std::process::exit(1);
    });
    let jobname = jobname.unwrap_or_else(|| job_of(&file));
    let job = Job {
        engine,
        ini,
        etex: etex || star,
        file: file.clone(),
        path,
        jobname,
        out_dir: out_dir.map_or_else(|| PathBuf::from("."), PathBuf::from),
        // (TeX's own default; with no terminal input an error stops the
        // job, as TeX's does when its standard input is at its end)
        threads,
        exact_pdf: false,
        synctex,
        recorder,
        shell_escape,
        pdf_date: session::pdf_date(&engine),
        // (one run, as the program's)
        fixpoint: false,
        cache_limit: None,
    };
    for n in session::waiting_notes(&job) {
        eprintln!("{progname}: {n}");
    }
    let status = std::sync::Arc::new(session::Status::default());
    let mut s = Session::new(job, status, &mut |n: &str| eprintln!("{progname}: {n}"))
        .unwrap_or_else(|e| {
            eprintln!("{progname}: {e}");
            std::process::exit(1);
        });
    crate::ui::capture_panics(false);
    let r = crate::ui::catch(|| s.build()).unwrap_or_else(|m| {
        eprintln!(
            "{progname}: phitex failed: {}",
            m.lines().next().unwrap_or("")
        );
        std::process::exit(crate::EXIT_INTERNAL);
    });
    print!("{}", Session::terminal(&r));
    if let Err(e) = s.write(&r, true) {
        eprintln!("{progname}: {e}");
        std::process::exit(1);
    }
    // web2c's `do_final_end`: exit status 1 for errors or worse.
    std::process::exit(i32::from(!r.out.errors.is_empty()));
}

/// The main file TeX finds for `name`: `name.tex`, else `name` (here, as
/// TeX's first `\input`).
pub fn find_main(name: &str) -> Option<PathBuf> {
    let with = format!("{name}.tex");
    let mut tries = Vec::new();
    if Path::new(name).extension().is_none_or(|e| e != "tex") {
        tries.push(with);
    }
    tries.push(name.to_owned());
    tries.into_iter().map(PathBuf::from).find(|p| p.is_file())
}

/// The job's name: the file's without its directory and `.tex`.
pub fn job_of(file: &str) -> String {
    let base = file.rsplit('/').next().unwrap_or(file);
    base.strip_suffix(".tex").unwrap_or(base).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn selection() {
        assert_eq!(
            select("/x/bin/pdflatex", v(&["a.tex"]), None),
            Selection::Compat {
                progname: "pdflatex".into(),
                args: v(&["a.tex"])
            }
        );
        assert_eq!(
            select("target/release/phitex", v(&["build", "x.tex"]), None),
            Selection::Modern(v(&["build", "x.tex"]))
        );
        assert_eq!(
            select("phitex", v(&["--compat=tex", "-ini", "x"]), None),
            Selection::Compat {
                progname: "tex".into(),
                args: v(&["-ini", "x"])
            }
        );
        assert_eq!(
            select("phitex", v(&["x"]), Some("latex")),
            Selection::Compat {
                progname: "latex".into(),
                args: v(&["x"])
            }
        );
        assert_eq!(job_of("dir/paper.tex"), "paper");
    }
}
