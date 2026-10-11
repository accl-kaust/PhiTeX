//! BibTeX and makeindex, in process (`partex-bibtex`, `partex-makeindex`):
//! between the runs of a build's fixpoint, as latexmk runs them, on the
//! `.aux` and `.idx` files the run wrote (held in memory, the build's
//! written values), their outputs (`.bbl`, `.ind`) given to the next run
//! as new versions of those files; and as the programs themselves when
//! phitex is run as `bibtex` or `makeindex`.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use partex_kpse::{Format, Kpse};

/// What a tool made for the build: the file the next run reads (`.bbl`,
/// `.ind`) and its text (TeX's characters), its other file (`.blg`,
/// `.ilg`), and a line for the user.
pub struct Made {
    pub file: String,
    pub text: Option<String>,
    pub side: Option<(String, Vec<u8>)>,
    pub line: String,
}

/// A lookup a run made and what it found, to know whether a run again
/// would read the same.
#[derive(Clone, PartialEq, Eq)]
enum Lookup {
    /// A file by name (relative names in the job's directory), its bytes.
    Read(Vec<u8>, Option<Vec<u8>>),
    /// A kpathsea lookup: the name, the format, the path and bytes found.
    Find(Vec<u8>, Format, Option<(Vec<u8>, Option<Vec<u8>>)>),
    /// `access(name, R_OK)`.
    Readable(Vec<u8>, bool),
    /// An `.aux` file's lines BibTeX reads.
    Aux(Vec<u8>, Option<Vec<u8>>),
}

/// Files for the tools: kpathsea's lookups, the files the run wrote
/// (`served`, by name), the rest from the job's directory `dir`.
struct ToolFiles<'a> {
    kpse: &'a mut Kpse,
    dir: PathBuf,
    served: &'a HashMap<String, Vec<u8>>,
    read: Vec<Lookup>,
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
        if n.is_absolute() { n.to_path_buf() } else { self.dir.join(n) }
    }

    fn get(&self, name: &[u8]) -> Option<Vec<u8>> {
        match self.served.get(&*String::from_utf8_lossy(name)) {
            Some(d) => Some(d.clone()),
            None => read_path(&self.at(name)),
        }
    }

    fn find(&mut self, name: &[u8], format: Format) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let found = self.kpse.find_file(name, format, true)?;
        let data = read_path(Path::new(std::ffi::OsStr::from_bytes(&found)));
        Some((found, data))
    }

    /// Whether every lookup of `read` finds the same now.
    fn unchanged(&mut self, read: &[Lookup]) -> bool {
        read.iter().all(|l| match l {
            Lookup::Read(n, was) => self.get(n) == *was,
            Lookup::Find(n, f, was) => self.find(n, *f) == *was,
            Lookup::Readable(n, was) => (self.served.contains_key(&*String::from_utf8_lossy(n))
                || self.at(n).exists())
                == *was,
            Lookup::Aux(n, was) => self.get(n).map(|d| aux_digest(&d)) == *was,
        })
    }
}

impl partex_bibtex::Files for ToolFiles<'_> {
    fn aux(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let d = self.get(name);
        self.read.push(Lookup::Aux(name.to_vec(), d.as_ref().map(|d| aux_digest(d))));
        d
    }

    fn bst(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let f = self.find(name, Format::Bst);
        self.read.push(Lookup::Find(name.to_vec(), Format::Bst, f.clone()));
        f.and_then(|(_, d)| d)
    }

    fn bib(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let f = self.find(name, Format::Bib);
        self.read.push(Lookup::Find(name.to_vec(), Format::Bib, f.clone()));
        f.and_then(|(_, d)| d)
    }
}

impl partex_makeindex::Files for ToolFiles<'_> {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let d = self.get(name);
        self.read.push(Lookup::Read(name.to_vec(), d.clone()));
        d
    }

    fn readable(&mut self, name: &[u8]) -> bool {
        let r = self.served.contains_key(&*String::from_utf8_lossy(name)) || self.at(name).exists();
        self.read.push(Lookup::Readable(name.to_vec(), r));
        r
    }

    fn style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let s = self.find(name, Format::Ist);
        self.read.push(Lookup::Find(name.to_vec(), Format::Ist, s.clone()));
        s
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

/// What BibTeX depends on in an `.aux` file: its `\citation`, `\bibdata`,
/// `\bibstyle` and `\@input` lines (as latexmk judges whether to rerun it).
fn aux_digest(data: &[u8]) -> Vec<u8> {
    let mut d = Vec::new();
    for line in data.split(|&c| c == b'\n') {
        if [&b"\\citation{"[..], b"\\bibdata{", b"\\bibstyle{", b"\\@input{"]
            .iter()
            .any(|p| line.starts_with(p))
        {
            d.extend_from_slice(line);
            d.push(b'\n');
        }
    }
    d
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

/// BibTeX's options, from `texmf.cnf` and the command line.
fn bib_options(kpse: &mut Kpse, terse: bool, min_crossrefs: i64) -> partex_bibtex::Options {
    partex_bibtex::Options {
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

/// The last non-empty line of a tool's terminal output.
fn summary(term: &[u8], failed: bool) -> String {
    let t = String::from_utf8_lossy(term);
    let last = t.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_owned();
    if failed { format!("failed: {last}") } else { last }
}

/// The tools of one build, across its runs (and a watch's builds): what
/// each run read, to run again only when that changed.
#[derive(Default)]
pub struct Tools {
    bib_kpse: Option<Kpse>,
    mi_kpse: Option<Kpse>,
    bib_last: HashMap<String, Vec<Lookup>>,
    mi_last: HashMap<String, Vec<Lookup>>,
    biber_noted: bool,
}

impl Tools {
    /// After a run that wrote `writes` (TeX's characters) in directory
    /// `dir`: BibTeX on each `.aux` file asking for it (`\bibdata`) and
    /// makeindex on each `.idx` file, unless what their last run read is
    /// the same. `known(name)` says whether the build has a version of
    /// the tool's output (else it runs).
    pub fn after_run(
        &mut self,
        writes: &[(String, String)],
        dir: &Path,
        known: &dyn Fn(&str) -> bool,
    ) -> Vec<Made> {
        let served: HashMap<String, Vec<u8>> =
            writes.iter().map(|(n, t)| (n.clone(), crate::tex::bytes(t))).collect();
        let mut made = Vec::new();
        for (name, text) in writes {
            if name.ends_with(".aux") && text.lines().any(|l| l.starts_with("\\bibdata{")) {
                let kpse = self.bib_kpse.get_or_insert_with(|| crate::tex::kpse("bibtex", ""));
                let bbl = format!("{}.bbl", name.trim_end_matches(".aux"));
                let mut files = ToolFiles { kpse, dir: dir.to_path_buf(), served: &served, read: Vec::new() };
                if known(&bbl) && self.bib_last.get(name).is_some_and(|r| files.unchanged(r)) {
                    continue;
                }
                let t = std::time::Instant::now();
                let opts = bib_options(files.kpse, false, 2);
                let out = partex_bibtex::run(name.as_bytes(), &opts, &mut files);
                self.bib_last.insert(name.clone(), std::mem::take(&mut files.read));
                made.push(Made {
                    file: out.bbl.as_ref().map_or(bbl, |f| String::from_utf8_lossy(&f.name).into_owned()),
                    text: out.bbl.as_ref().map(|f| crate::tex::chars(&f.contents)),
                    side: out.blg.as_ref().map(|f| (String::from_utf8_lossy(&f.name).into_owned(), f.contents.clone())),
                    line: format!(
                        "bibtex {name} in {:.1} ms: {}",
                        t.elapsed().as_secs_f64() * 1e3,
                        summary(&out.term, out.status != 0)
                    ),
                });
            }
            if name.ends_with(".idx") {
                let kpse = self.mi_kpse.get_or_insert_with(|| crate::tex::kpse("makeindex", ""));
                let ind = format!("{}.ind", name.trim_end_matches(".idx"));
                let mut files = ToolFiles { kpse, dir: dir.to_path_buf(), served: &served, read: Vec::new() };
                if known(&ind) && self.mi_last.get(name).is_some_and(|r| files.unchanged(r)) {
                    continue;
                }
                let t = std::time::Instant::now();
                let out = partex_makeindex::run(&[name.as_bytes().to_vec()], &mi_version(), &mut files);
                self.mi_last.insert(name.clone(), std::mem::take(&mut files.read));
                made.push(Made {
                    file: out.ind.as_ref().map_or(ind, |f| String::from_utf8_lossy(&f.name).into_owned()),
                    text: out.ind.as_ref().map(|f| crate::tex::chars(&f.contents)),
                    side: out.ilg.as_ref().map(|f| (String::from_utf8_lossy(&f.name).into_owned(), f.contents.clone())),
                    line: format!(
                        "makeindex {name} in {:.1} ms: {}",
                        t.elapsed().as_secs_f64() * 1e3,
                        summary(&out.stderr, out.status != 0)
                    ),
                });
            }
            if name.ends_with(".bcf") && !self.biber_noted {
                self.biber_noted = true;
                made.push(Made {
                    file: String::new(),
                    text: None,
                    side: None,
                    line: format!(
                        "biber: not run for {name}: the native biber is planned, not there yet \
                         (biblatex shows the citation keys)"
                    ),
                });
            }
        }
        made
    }
}

/// The tools as the build's own derivation (`Build::set_derive`): BibTeX
/// and makeindex inside the build's loop-back, on the files each
/// evaluation wrote; their outputs are files the next evaluation reads,
/// and only the readers of what changed run again.
pub struct ToolDerive {
    pub tools: Tools,
    /// The job's directory (relative names of the files the tools read).
    pub dir: PathBuf,
}

impl phitex_flow::build::Derive for ToolDerive {
    fn derive(&mut self, writes: &[(String, String)], known: &dyn Fn(&str) -> bool) -> Vec<phitex_flow::build::Derived> {
        let dir = self.dir.clone();
        self.tools
            .after_run(writes, &dir, &|n: &str| known(n) || dir.join(n).is_file())
            .into_iter()
            .map(|m| phitex_flow::build::Derived {
                bytes: m.text.as_deref().map(crate::tex::bytes),
                name: m.file,
                side: m.side,
                note: m.line,
            })
            .collect()
    }
}

/// Whether this invocation is BibTeX (`bibtex`, or `phitex -bibtex`).
pub fn bibtex_wanted(argv0: &str, args: &[String]) -> bool {
    stem(argv0) == "bibtex" || args.first().is_some_and(|a| a == "-bibtex" || a == "--bibtex")
}

/// Whether this invocation is makeindex.
pub fn makeindex_wanted(argv0: &str, args: &[String]) -> bool {
    stem(argv0) == "makeindex" || args.first().is_some_and(|a| a == "-makeindex" || a == "--makeindex")
}

fn stem(argv0: &str) -> String {
    Path::new(argv0).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Run as `bibtex [-terse] [-min-crossrefs=N] AUX`.
pub fn bibtex_main(args: &[String]) -> ! {
    let mut terse = false;
    let mut min_crossrefs = 2;
    let mut files = Vec::new();
    let mut it = args.iter().skip_while(|a| *a == "-bibtex" || *a == "--bibtex").peekable();
    while let Some(a) = it.next() {
        let opt = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
        match opt {
            Some("terse") => terse = true,
            Some("min-crossrefs") => min_crossrefs = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
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
        eprintln!("bibtex: Need exactly one file argument.\nTry `bibtex --help' for more information.");
        std::process::exit(1);
    }
    let mut kpse = crate::tex::kpse("bibtex", "");
    let opts = bib_options(&mut kpse, terse, min_crossrefs);
    let served = HashMap::new();
    let mut fs = ToolFiles { kpse: &mut kpse, dir: PathBuf::from("."), served: &served, read: Vec::new() };
    let out = partex_bibtex::run(files[0].as_bytes(), &opts, &mut fs);
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
    let served = HashMap::new();
    let mut fs = ToolFiles { kpse: &mut kpse, dir: PathBuf::from("."), served: &served, read: Vec::new() };
    let out = partex_makeindex::run(&args, &mi_version(), &mut fs);
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
