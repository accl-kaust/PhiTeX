//! BibTeX and makeindex as the programs themselves, when phitex is run as
//! `bibtex` or `makeindex` (`phitex-bibtex`, `phitex-makeindex`), and
//! BibTeX's settings from `texmf.cnf`. A build runs the tools itself where
//! its reads ask for their outputs (`phitex_tex`'s tools), on the files the
//! job wrote, their support files found by [`crate::tex::Tree::tool_input`].

use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use phitex_kpse::{Format, Kpse};

/// Files for the tools run as programs: kpathsea's lookups, the rest from
/// the job's directory `dir`.
struct ToolFiles<'a> {
    kpse: &'a mut Kpse,
    dir: PathBuf,
}

fn read_path(p: &Path) -> Option<Vec<u8>> {
    if p.is_dir() {
        return None;
    }
    std::fs::read(p).ok()
}

impl ToolFiles<'_> {
    fn at(&self, name: &[u8]) -> PathBuf {
        let n = Path::new(std::ffi::OsStr::from_bytes(name));
        if n.is_absolute() {
            n.to_path_buf()
        } else {
            self.dir.join(n)
        }
    }

    fn get(&self, name: &[u8]) -> Option<Vec<u8>> {
        read_path(&self.at(name))
    }

    fn find(&mut self, name: &[u8], format: Format) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let found = self.kpse.find_file(name, format, true)?;
        let data = read_path(Path::new(std::ffi::OsStr::from_bytes(&found)));
        Some((found, data))
    }
}

impl phitex_bibtex::Files for ToolFiles<'_> {
    fn aux(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.get(name)
    }

    fn bst(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.find(name, Format::Bst).and_then(|(_, d)| d)
    }

    fn bib(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.find(name, Format::Bib).and_then(|(_, d)| d)
    }
}

impl phitex_makeindex::Files for ToolFiles<'_> {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.get(name)
    }

    fn readable(&mut self, name: &[u8]) -> bool {
        self.at(name).exists()
    }

    fn style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        self.find(name, Format::Ist)
    }

    fn out_name_ok(&mut self, _name: &[u8]) -> Result<(), Vec<u8>> {
        Ok(())
    }

    fn stdin(&mut self) -> Vec<u8> {
        let mut v = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut v);
        v
    }
}

/// bibtex.ch's `setup_params`: a capacity from `texmf.cnf`, at least its
/// built-in value.
fn bound(kpse: &mut Kpse, name: &str, default: i64) -> i64 {
    kpse.var_value(name)
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(default)
        .max(default)
}

/// BibTeX's options, from `texmf.cnf` and the command line (a build's
/// BibTeX runs: [`phitex_tex::Options::bibtex`]).
pub fn bib_options(kpse: &mut Kpse, terse: bool, min_crossrefs: i64) -> phitex_bibtex::Options {
    phitex_bibtex::Options {
        terse,
        min_crossrefs,
        max_strings: bound(kpse, "max_strings", 4000),
        ent_str_size: usize::try_from(bound(kpse, "ent_str_size", 100)).unwrap_or(100),
        glob_str_size: usize::try_from(bound(kpse, "glob_str_size", 1000)).unwrap_or(1000),
        max_print_line: usize::try_from(bound(kpse, "max_print_line", 60)).unwrap_or(60),
        version: env!("PHITEX_ORACLE_VERSION_SUFFIX").as_bytes().to_vec(),
    }
}

/// makeindex's banner: 2.18 as TeX Live builds it.
fn mi_version() -> Vec<u8> {
    let suffix = env!("PHITEX_ORACLE_VERSION_SUFFIX");
    let year: String = suffix
        .split_once("TeX Live ")
        .map(|(_, r)| r.chars().take_while(char::is_ascii_digit).collect())
        .unwrap_or_default();
    format!("version 2.18 [TeX Live {year}] (kpathsea + Thai support)").into_bytes()
}

/// Whether this invocation is BibTeX (`bibtex`, or `phitex -bibtex`).
pub fn bibtex_wanted(argv0: &str, args: &[String]) -> bool {
    stem(argv0) == "bibtex"
        || args
            .first()
            .is_some_and(|a| a == "-bibtex" || a == "--bibtex")
}

/// Whether this invocation is makeindex.
pub fn makeindex_wanted(argv0: &str, args: &[String]) -> bool {
    stem(argv0) == "makeindex"
        || args
            .first()
            .is_some_and(|a| a == "-makeindex" || a == "--makeindex")
}

fn stem(argv0: &str) -> String {
    Path::new(argv0)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Run as `bibtex [-terse] [-min-crossrefs=N] AUX`.
pub fn bibtex_main(args: &[String]) -> ! {
    let mut terse = false;
    let mut min_crossrefs = 2;
    let mut files = Vec::new();
    let mut it = args
        .iter()
        .skip_while(|a| *a == "-bibtex" || *a == "--bibtex")
        .peekable();
    while let Some(a) = it.next() {
        let opt = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
        match opt {
            Some("terse") => terse = true,
            Some("min-crossrefs") => {
                min_crossrefs = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            Some(o) if o.starts_with("min-crossrefs=") => {
                min_crossrefs = o["min-crossrefs=".len()..].parse().unwrap_or(0);
            }
            Some("version") => {
                println!("BibTeX 0.99e{}", env!("PHITEX_ORACLE_VERSION_SUFFIX"));
                std::process::exit(0);
            }
            Some(_) if a.len() > 1 => {
                eprintln!("Try `bibtex --help' for more information.");
                std::process::exit(1);
            }
            _ => files.push(a.clone()),
        }
    }
    if files.len() != 1 {
        eprintln!(
            "bibtex: Need exactly one file argument.\nTry `bibtex --help' for more information."
        );
        std::process::exit(1);
    }
    let mut kpse = crate::tex::kpse("bibtex", "");
    let opts = bib_options(&mut kpse, terse, min_crossrefs);
    let mut fs = ToolFiles {
        kpse: &mut kpse,
        dir: PathBuf::from("."),
    };
    let out = phitex_bibtex::run(files[0].as_bytes(), &opts, &mut fs);
    for f in [&out.blg, &out.bbl].into_iter().flatten() {
        let name = String::from_utf8_lossy(&f.name).into_owned();
        if let Err(e) = std::fs::write(&name, &f.contents) {
            eprintln!("bibtex: can't write {name}: {e}");
            std::process::exit(1);
        }
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&out.term);
    let _ = stdout.flush();
    std::process::exit(out.status);
}

/// Run as `makeindex [options] IDX...`.
pub fn makeindex_main(args: &[String]) -> ! {
    let args: Vec<Vec<u8>> = args
        .iter()
        .skip_while(|a| *a == "-makeindex" || *a == "--makeindex")
        .map(|a| a.as_bytes().to_vec())
        .collect();
    let mut kpse = crate::tex::kpse("makeindex", "");
    let mut fs = ToolFiles {
        kpse: &mut kpse,
        dir: PathBuf::from("."),
    };
    let out = phitex_makeindex::run(&args, &mi_version(), &mut fs);
    let mut failed = false;
    for f in [&out.ind, &out.ilg].into_iter().flatten() {
        let name = String::from_utf8_lossy(&f.name).into_owned();
        if let Err(e) = std::fs::write(&name, &f.contents) {
            eprintln!("makeindex: can't write {name}: {e}");
            failed = true;
        }
    }
    let _ = std::io::stdout().write_all(&out.stdout);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().write_all(&out.stderr);
    std::process::exit(if failed { 1 } else { out.status });
}
