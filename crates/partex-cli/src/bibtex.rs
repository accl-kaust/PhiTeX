//! `partex -bibtex [-terse] [-min-crossrefs=N] AUX` (or partex invoked as
//! `bibtex`): BibTeX, in process (`partex-bibtex`), with kpathsea's
//! lookups and `texmf.cnf` capacities, writing the `.bbl` and `.blg` files
//! and the transcript as `bibtex` would.

use std::collections::HashMap;
use std::io::Write;

use partex_bibtex::{Files, Options};
use partex_kpse::{Format, Kpse};

/// Whether this invocation asks for BibTeX.
pub fn wanted() -> bool {
    let argv0 = std::env::args().next().unwrap_or_default();
    std::path::Path::new(&argv0)
        .file_stem()
        .is_some_and(|s| s == "bibtex")
        || std::env::args()
            .nth(1)
            .is_some_and(|a| a == "-bibtex" || a == "--bibtex")
}

/// A lookup: the name asked for, and what it found (the file's name and
/// what BibTeX depends on in it).
type Lookup = (Vec<u8>, Option<(Vec<u8>, Vec<u8>)>);

/// kpathsea lookups for the style and databases; `.aux` files as named.
pub struct KpseFiles {
    pub kpse: Kpse,
    /// Every lookup made.
    pub read: Vec<Lookup>,
    /// The directory the run acts as if started in (`cd dir && bibtex
    /// job`, as latexmk runs it for an output directory): `.aux`, `.bbl`
    /// and `.blg` names are in it.
    dir: Option<Vec<u8>>,
    /// `.aux` files read from these contents, by path, not from disk: the
    /// streams the build just wrote, which an SSA build holds in memory
    /// while it rewrites the files (DESIGN 3.7, "Trips, as built").
    served: HashMap<Vec<u8>, Vec<u8>>,
}

/// Read `name` (not a directory).
fn read_file(name: &[u8]) -> Option<Vec<u8>> {
    let p = crate::native::path(name);
    if std::fs::metadata(&p).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    std::fs::read(&p).ok()
}

/// What BibTeX depends on in an `.aux` file: its `\citation`, `\bibdata`,
/// `\bibstyle` and `\@input` lines (as latexmk judges whether to rerun it).
fn aux_digest(data: &[u8]) -> Vec<u8> {
    let mut d = Vec::new();
    for line in data.split(|&c| c == b'\n') {
        if [
            &b"\\citation{"[..],
            b"\\bibdata{",
            b"\\bibstyle{",
            b"\\@input{",
        ]
        .iter()
        .any(|p| line.starts_with(p))
        {
            d.extend_from_slice(line);
            d.push(b'\n');
        }
    }
    d
}

impl KpseFiles {
    /// The `.aux` file at `name`: served, or read from disk.
    fn read_aux(&self, name: &[u8]) -> Option<Vec<u8>> {
        match self.served.get(name) {
            Some(d) => Some(d.clone()),
            None => read_file(name),
        }
    }

    /// Where `name` is, relative names being in [`KpseFiles::dir`].
    fn at(&self, name: &[u8]) -> Vec<u8> {
        match &self.dir {
            Some(d) if name.first() != Some(&b'/') => [d.as_slice(), name].concat(),
            _ => name.to_vec(),
        }
    }

    fn find(&mut self, name: &[u8], format: Format) -> Option<Vec<u8>> {
        let found = self
            .kpse
            .find_file(name, format, true)
            .and_then(|f| Some((read_file(&f)?, f)));
        self.read.push((
            name.to_vec(),
            found.as_ref().map(|(d, f)| (f.clone(), d.clone())),
        ));
        found.map(|(d, _)| d)
    }

    /// Whether every lookup in `read` finds the same now.
    fn unchanged(&mut self, read: &[Lookup]) -> bool {
        read.iter().all(|(name, was)| {
            let now = if name.ends_with(b".aux") {
                self.read_aux(name).map(|d| (name.clone(), aux_digest(&d)))
            } else {
                let format = if was.as_ref().is_some_and(|(f, _)| f.ends_with(b".bst")) {
                    Format::Bst
                } else {
                    Format::Bib
                };
                self.kpse
                    .find_file(name, format, true)
                    .and_then(|f| Some((f.clone(), read_file(&f)?)))
            };
            now == *was
        })
    }
}

impl Files for KpseFiles {
    fn aux(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let name = self.at(name);
        let data = self.read_aux(&name);
        self.read
            .push((name.clone(), data.as_ref().map(|d| (name, aux_digest(d)))));
        data
    }

    fn bst(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.find(name, Format::Bst)
    }

    fn bib(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.find(name, Format::Bib)
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

/// The run's options, from `texmf.cnf` and the command line.
pub fn options(kpse: &mut Kpse, terse: bool, min_crossrefs: i64) -> Options {
    Options {
        terse,
        min_crossrefs,
        max_strings: bound(kpse, "max_strings", 4000),
        ent_str_size: usize::try_from(bound(kpse, "ent_str_size", 100)).unwrap_or(100),
        glob_str_size: usize::try_from(bound(kpse, "glob_str_size", 1000)).unwrap_or(1000),
        max_print_line: usize::try_from(bound(kpse, "max_print_line", 60)).unwrap_or(60),
        version: env!("PARTEX_ORACLE_VERSION_SUFFIX").as_bytes().to_vec(),
    }
}

fn usage() -> ! {
    eprintln!("Try `bibtex --help' for more information.");
    std::process::exit(1);
}

/// BibTeX runs made by a converging build, remembered across its passes
/// (and a resident session's requests).
#[derive(Default)]
pub struct Runs {
    files: Option<KpseFiles>,
    opts: Option<Options>,
    /// For each `.aux` file, the lookups its last run made.
    last: HashMap<Vec<u8>, Vec<Lookup>>,
}

/// After a pass of a converging build (its outputs written): run BibTeX
/// on each `.aux` file of the job that asks for it (`\bibdata`), unless
/// what its last run read is unchanged, writing the `.bbl` and `.blg`
/// files as `bibtex` would. The `.aux` files are read from `auxes`, by
/// path (the others, from disk). A report line for each run.
pub fn after_pass(runs: &mut Runs, auxes: &[(Vec<u8>, Vec<u8>)]) -> Vec<String> {
    let mut reports = Vec::new();
    let served: HashMap<Vec<u8>, Vec<u8>> = auxes.iter().cloned().collect();
    for (name, contents) in auxes {
        if !contents
            .split(|&c| c == b'\n')
            .any(|l| l.starts_with(b"\\bibdata{"))
        {
            continue;
        }
        let files = runs.files.get_or_insert_with(|| KpseFiles {
            kpse: crate::kpse_instance("bibtex", ""),
            read: Vec::new(),
            dir: None,
            served: HashMap::new(),
        });
        files.served.clone_from(&served);
        let slash = name.iter().rposition(|&c| c == b'/');
        files.dir = slash.map(|i| name[..=i].to_vec());
        let base = slash.map_or(name.as_slice(), |i| &name[i + 1..]).to_vec();
        let opts = runs
            .opts
            .get_or_insert_with(|| options(&mut files.kpse, false, 2))
            .clone();
        if let Some(read) = runs.last.get(name)
            && files.unchanged(read)
        {
            continue;
        }
        let t0 = std::time::Instant::now();
        let _p = crate::timeline::phase("bibtex").map(|mut p| {
            p.detail(String::from_utf8_lossy(name));
            p
        });
        files.read.clear();
        let out = partex_bibtex::run(&base, &opts, files);
        runs.last
            .insert(name.clone(), std::mem::take(&mut files.read));
        for f in [&out.blg, &out.bbl].into_iter().flatten() {
            if let Err(e) = std::fs::write(crate::native::path(&files.at(&f.name)), &f.contents) {
                reports.push(format!(
                    "partex: bibtex: can't write {}: {e}",
                    String::from_utf8_lossy(&f.name)
                ));
            }
        }
        let last_line = crate::native::tool_summary(&out.term, out.status != 0);
        reports.push(format!(
            "partex: bibtex {} in {:.1} ms: {}",
            String::from_utf8_lossy(name),
            t0.elapsed().as_secs_f64() * 1e3,
            last_line
        ));
    }
    reports
}

/// Run as `bibtex`.
pub fn main() -> ! {
    let mut terse = false;
    let mut min_crossrefs = 2;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1).peekable();
    if args
        .peek()
        .is_some_and(|a| a == "-bibtex" || a == "--bibtex")
    {
        args.next();
    }
    while let Some(a) = args.next() {
        let opt = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
        match opt {
            Some("terse") => terse = true,
            Some("min-crossrefs") => {
                min_crossrefs = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            }
            Some(o) if o.starts_with("min-crossrefs=") => {
                min_crossrefs = o["min-crossrefs=".len()..].parse().unwrap_or(0);
            }
            Some("version") => {
                println!("BibTeX 0.99e{}", env!("PARTEX_ORACLE_VERSION_SUFFIX"));
                std::process::exit(0);
            }
            Some(_) if a.len() > 1 => usage(),
            _ => files.push(a),
        }
    }
    if files.len() != 1 {
        eprintln!("bibtex: Need exactly one file argument.");
        usage();
    }
    let mut kpse = crate::kpse_instance("bibtex", "");
    let opts = options(&mut kpse, terse, min_crossrefs);
    let mut fs = KpseFiles {
        kpse,
        read: Vec::new(),
        dir: None,
        served: HashMap::new(),
    };
    let out = partex_bibtex::run(files[0].as_bytes(), &opts, &mut fs);
    for f in [&out.blg, &out.bbl].into_iter().flatten() {
        if let Err(e) = std::fs::write(crate::native::path(&f.name), &f.contents) {
            eprintln!(
                "bibtex: can't write {}: {e}",
                String::from_utf8_lossy(&f.name)
            );
            std::process::exit(1);
        }
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&out.term);
    let _ = stdout.flush();
    std::process::exit(out.status);
}
