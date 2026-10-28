//! What loading a package reads on PhiTeX, for Shelf's dependency
//! columns: for each name one job (`\documentclass{article}\usepackage{X}`
//! for a .sty, `\documentclass{X}` for a .cls, and a body that typesets
//! the shapes a paper uses), built by `phitex_tex::Build`, and every file
//! the build's finders gave it printed:
//!
//! ```text
//! shelf-trace --version                 # phitex <commit> shelf-trace <N>
//! shelf-trace --engine pdftex|xetex --flat DIR [--prelude FILE]
//!             [--font-index FILE] NAME...      # NAME <TAB> file,file,...
//! ```
//!
//! A line per name as soon as it is done (tracer.py runs batches with a
//! timeout: a job that never ends costs its batch, not the trace).
//!
//! What a job reads is what the build asks its finders for and gets: the
//! document's finder (`Options::finder`: `\input`, `\openin`, TFM, VF,
//! the map, Type 1 fonts and encodings for the PDF) over DIR, a flat
//! directory of TeX Live's files as Shelf's search order finds them by
//! name (build.py writes it), and under XeTeX the native fonts
//! (`XeTeX::fonts`: a `[file]` name from DIR; a face the font index names,
//! by its path, printed as its texmf path `fonts/opentype/...`). That is
//! exactly what a client serving the same build from Shelf is asked for:
//! the build's own query, not a log or a recorder file after the fact.
//!
//! The format (the prelude, `\input pdflatex.ini` or `xelatex.ini`) is
//! TeX Live's, through kpathsea (a TeX Live on PATH), and its reads are
//! not printed: every document has them (the old trace's `.fmt`). Its
//! state is made once and kept in `--prelude FILE` when given.
//! `SHELF_TRACE_LOGS=DIR`: the TeX log of each job that made no page,
//! in `DIR/<engine>/<name>.log`.
//! `--font-index` is `phitex-otf`'s font index (`otf-index`), which
//! XeTeX's font names are looked up in. TECkit mappings (fonts/misc, not
//! on Shelf) come from TeX Live. XeTeX's XDV is not made a PDF: what
//! xdvipdfmx reads is not in the column (as before).

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use phitex_kpse::{Format, Kpse};
use phitex_tex::{Build, FontIndex, FontSource, Finder, KpseFormat, Options, Severity, XeTeX};

/// The output's version: bumped when what a line means changes.
const PROTOCOL: u32 = 1;

/// A body that uses the shapes a paper does: the fonts a package brings
/// are read then, so they arrive with it (the old trace-deps' body).
const BODY: &str = "\\begin{document}\\section{S}x \\textbf{b} \\textit{i} \\emph{e} \\textsc{s} \\texttt{t} \\textsf{f} \\textbf{\\textit{bi}} {\\small s}{\\footnotesize f}{\\large l} $x^2+\\alpha\\sum_{i=1}^n\\int f\\to\\mathbb{R}$ \\[\\sum_i x_i\\]\\end{document}\n";

type Read = Arc<Mutex<BTreeSet<String>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn usage() -> ! {
    eprintln!(
        "usage: shelf-trace --version\n       shelf-trace --engine pdftex|xetex --flat DIR [--prelude FILE] [--font-index FILE] NAME..."
    );
    std::process::exit(2);
}

/// kpathsea as `progname`, from the `tex` on PATH (TeX Live's tree).
fn kpse(progname: &str, engine: &str) -> Kpse {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dir = std::env::split_paths(&path)
        .find(|d| d.join("tex").is_file())
        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
    Kpse::new(&dir, progname, engine)
}

/// The kpathsea format a name's extension names.
fn format_of(name: &str) -> Format {
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "tfm" => Format::Tfm,
        "vf" => Format::Vf,
        "map" => Format::FontMap,
        "pfb" | "pfa" => Format::Type1,
        "enc" => Format::Enc,
        "otf" => Format::OpenType,
        "ttf" | "ttc" => Format::TrueType,
        _ => Format::Tex,
    }
}

/// The format's finder: TeX Live's, without the current directory (as
/// fmtutil builds it), nothing recorded.
fn prelude_finder(progname: &str, engine: &str) -> Finder {
    let mut k = kpse(progname, engine);
    for f in [
        Format::Tex,
        Format::Tfm,
        Format::Vf,
        Format::FontMap,
        Format::Type1,
        Format::Enc,
        Format::OpenType,
        Format::TrueType,
    ] {
        k.drop_cwd(f);
    }
    let k = Mutex::new(k);
    Arc::new(move |name: &str| {
        let p = lock(&k).find_file(name.as_bytes(), format_of(name), false)?;
        let p = String::from_utf8(p).ok()?;
        let b = std::fs::read(&p).ok()?;
        Some((p, b))
    })
}

/// A name the flat directory may have: a base name, no dot file.
fn flat_name(name: &str) -> Option<&str> {
    let n = name.strip_prefix("./").unwrap_or(name);
    (!n.is_empty() && !n.contains('/') && !n.starts_with('.')).then_some(n)
}

/// The job's finder: the flat directory by name, every hit recorded.
fn job_finder(flat: PathBuf, read: Read) -> Finder {
    Arc::new(move |name: &str| {
        let n = flat_name(name)?;
        let p = flat.join(n);
        let b = std::fs::read(&p).ok()?;
        lock(&read).insert(n.to_owned());
        Some((p.to_string_lossy().into_owned(), b))
    })
}

/// XeTeX's font files: a `[file]` name from the flat directory, a face
/// the index names by its path (recorded as its texmf path), TECkit
/// mappings from TeX Live.
struct Fonts {
    flat: PathBuf,
    kpse: Mutex<Kpse>,
    read: Read,
}

/// A TeX Live path's texmf path (`.../texmf-dist/fonts/opentype/x.otf`
/// is `fonts/opentype/x.otf`).
fn texmf_path(p: &str) -> Option<&str> {
    p.find("/fonts/").map(|i| &p[i + 1..])
}

impl FontSource for Fonts {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        let p = Path::new(path);
        if let Ok(rest) = p.strip_prefix(&self.flat) {
            let b = std::fs::read(p).ok()?;
            lock(&self.read).insert(rest.to_string_lossy().into_owned());
            return Some(Arc::from(b));
        }
        let b = std::fs::read(p).ok()?;
        if let Some(t) = texmf_path(path).filter(|t| t.starts_with("fonts/opentype/") || t.starts_with("fonts/truetype/")) {
            lock(&self.read).insert(t.to_owned());
        }
        Some(Arc::from(b))
    }

    fn find_file(&self, name: &str, format: KpseFormat) -> Option<String> {
        // (kpathsea's suffixes: a name without one of the format's is
        // looked for with each, then as it is: `[lmroman10-regular]`)
        let sfx: &[&str] = match format {
            KpseFormat::OpenType => &[".otf"],
            KpseFormat::TrueType => &[".ttf", ".ttc"],
            KpseFormat::Type1 => &[".pfb", ".pfa"],
            KpseFormat::MiscFonts => &[],
        };
        if let Some(n) = flat_name(name).filter(|_| format != KpseFormat::MiscFonts) {
            let has = sfx.iter().any(|s| n.to_ascii_lowercase().ends_with(s));
            let tries = sfx.iter().filter(|_| !has).map(|s| format!("{n}{s}")).chain([n.to_owned()]);
            for t in tries {
                if self.flat.join(&t).is_file() {
                    return Some(self.flat.join(t).to_string_lossy().into_owned());
                }
            }
        }
        let f = match format {
            KpseFormat::OpenType => Format::OpenType,
            KpseFormat::TrueType => Format::TrueType,
            KpseFormat::Type1 => Format::Type1,
            KpseFormat::MiscFonts => Format::MiscFonts,
        };
        // (TECkit's mappings: TeX Live's, never Shelf's; a font not on
        // Shelf is not given, as a client could not)
        if format != KpseFormat::MiscFonts {
            return None;
        }
        let p = lock(&self.kpse).find_file(name.as_bytes(), f, false)?;
        String::from_utf8(p).ok()
    }
}

#[allow(clippy::too_many_lines, reason = "one loop, in order")]
fn main() {
    let mut args = std::env::args().skip(1);
    let (mut engine, mut flat, mut prelude_file, mut index_file, mut names) =
        (String::new(), None, None, None, Vec::new());
    while let Some(a) = args.next() {
        match a.as_str() {
            "--version" => {
                println!("phitex {} shelf-trace {PROTOCOL}", env!("PHITEX_COMMIT"));
                return;
            }
            "--engine" => engine = args.next().unwrap_or_else(|| usage()),
            "--flat" => flat = args.next().map(PathBuf::from),
            "--prelude" => prelude_file = args.next().map(PathBuf::from),
            "--font-index" => index_file = args.next().map(PathBuf::from),
            n if !n.starts_with("--") => names.push(n.to_owned()),
            _ => usage(),
        }
    }
    let Some(flat) = flat else { usage() };
    let flat = std::fs::canonicalize(&flat).unwrap_or(flat);
    let xe = match engine.as_str() {
        "pdftex" => false,
        "xetex" => true,
        _ => usage(),
    };
    let (progname, kengine, ini) = if xe {
        ("xelatex", "xetex", "\\input xelatex.ini\n")
    } else {
        ("pdflatex", "pdftex", "\\input pdflatex.ini\n")
    };
    let index = index_file.map(|p| {
        let b = std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        Arc::new(FontIndex::from_bytes(&b).unwrap_or_else(|| panic!("{}: not a font index", p.display())))
    });
    if xe && index.is_none() {
        eprintln!("shelf-trace: xetex without --font-index: no font is found by name");
    }
    let pfinder = prelude_finder(progname, kengine);
    let mut prelude_bytes = prelude_file.as_ref().and_then(|p| std::fs::read(p).ok());
    let mut out = std::io::stdout().lock();
    for name in &names {
        let Some((stem, kind)) = name.rsplit_once('.') else { continue };
        let job = match kind {
            "sty" => format!("\\nonstopmode\\documentclass{{article}}\\usepackage{{{stem}}}{BODY}"),
            "cls" => format!("\\nonstopmode\\documentclass{{{stem}}}{BODY}"),
            _ => continue,
        };
        let t0 = Instant::now();
        let read: Read = Arc::default();
        let opts = Options {
            name: "./phitexdepsjob.tex".to_owned(),
            jobname: "phitexdepsjob".to_owned(),
            finder: job_finder(flat.clone(), read.clone()),
            prelude_finder: Some(pfinder.clone()),
            prelude_jobname: progname.to_owned(),
            prelude_bytes: prelude_bytes.clone(),
            epoch: Some(0),
            threads: Some(1),
            time_limit: Some(Duration::from_secs(120)),
            stuck_after: Some(Duration::from_secs(60)),
            xetex: xe.then(|| XeTeX {
                fonts: Some(Arc::new(Fonts {
                    flat: flat.clone(),
                    kpse: Mutex::new(kpse(progname, kengine)),
                    read: read.clone(),
                })),
                font_index: index.clone(),
            }),
            ..Options::default()
        };
        let b = Build::with(opts, Some(ini), &job);
        // (the format's state, made by the first job: kept for the next
        // process; this one has it in memory)
        if prelude_bytes.is_none()
            && let Some(state) = b.export_prelude()
        {
            if let Some(p) = &prelude_file {
                let tmp = p.with_extension(format!("part-{}", std::process::id()));
                if std::fs::write(&tmp, &state).is_ok() {
                    let _ = std::fs::rename(&tmp, p);
                }
            }
            prelude_bytes = Some(state);
        }
        let errors: Vec<String> = b
            .diagnostics()
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error))
            .map(|d| d.message.lines().next().unwrap_or("").chars().take(120).collect())
            .collect();
        let pages = b.pages().len();
        let files: Vec<String> = lock(&read).iter().cloned().collect();
        // (SHELF_TRACE_LOGS=DIR: the log of a job that made no page, in
        // DIR/<engine>/<name>.log: why a load stopped short)
        if pages == 0
            && let Some(dir) = std::env::var_os("SHELF_TRACE_LOGS")
        {
            let d = Path::new(&dir).join(&engine);
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join(format!("{name}.log")), b.output().1);
        }
        drop(b);
        let _ = writeln!(out, "{name}\t{}", files.join(","));
        let _ = out.flush();
        // (the log's line per job: the first errors say why a load stopped
        // short, a missing file or a font not found)
        let mut first: Vec<&String> = Vec::new();
        for e in &errors {
            if !first.contains(&e) && first.len() < 3 {
                first.push(e);
            }
        }
        eprintln!(
            "{name}: {} files, {pages} pages, {} errors, {:.2} s{}{}",
            files.len(),
            errors.len(),
            t0.elapsed().as_secs_f64(),
            if first.is_empty() { "" } else { ": " },
            first.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" | ")
        );
    }
}
